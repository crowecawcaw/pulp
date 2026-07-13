mod common;

use pulp::db::repos::collector_target::{target_id, SqliteCollectorTargetRepo};
use pulp::db::repos::traits::CollectorTargetRepo;
use serial_test::serial;

// ── Repo tests (direct, in-memory pool) ─────────────────────────────────────

async fn repo() -> SqliteCollectorTargetRepo {
    let db_name = uuid::Uuid::new_v4().to_string().replace('-', "");
    let url = format!("sqlite:file:{}?mode=memory&cache=shared", db_name);
    // Keep a connection alive for the shared in-memory db for the test's life by
    // leaking the pool used for migration — simplest for a unit-style test.
    let pool = pulp::db::pool::create_pool(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    Box::leak(Box::new(pool.clone()));
    SqliteCollectorTargetRepo::new(pool)
}

#[tokio::test]
async fn upsert_get_list_targets() {
    let repo = repo().await;
    let t = repo
        .upsert_target("reddit", "search", "\"a\" OR \"b\"")
        .await
        .unwrap();
    assert_eq!(t.id, target_id("reddit", "search", "\"a\" OR \"b\""));
    assert_eq!(t.consecutive_failures, 0);
    assert!(t.confirmed_watermark.is_none());

    // Idempotent: re-upsert returns the same row, preserves status.
    repo.record_target_success(&t.id, Some(1000)).await.unwrap();
    let again = repo
        .upsert_target("reddit", "search", "\"a\" OR \"b\"")
        .await
        .unwrap();
    assert_eq!(again.id, t.id);
    assert_eq!(
        again.confirmed_watermark,
        Some(1000),
        "upsert preserves status"
    );

    repo.upsert_target("reddit", "feed", "rust+golang")
        .await
        .unwrap();
    repo.upsert_target("github", "search", "x").await.unwrap();
    let reddit = repo.list_targets("reddit").await.unwrap();
    assert_eq!(reddit.len(), 2);
    assert_eq!(repo.list_targets("github").await.unwrap().len(), 1);
}

#[tokio::test]
async fn success_clears_failure_is_sticky() {
    let repo = repo().await;
    let t = repo.upsert_target("reddit", "search", "q").await.unwrap();

    repo.record_target_failure(&t.id, "boom").await.unwrap();
    let f = repo.get_target(&t.id).await.unwrap().unwrap();
    assert_eq!(f.consecutive_failures, 1);
    assert_eq!(f.last_error.as_deref(), Some("boom"));

    // A second failure increments (sticky accumulation).
    repo.record_target_failure(&t.id, "boom2").await.unwrap();
    assert_eq!(
        repo.get_target(&t.id)
            .await
            .unwrap()
            .unwrap()
            .consecutive_failures,
        2
    );

    // Success clears failure/error and advances watermark.
    let before = chrono::Utc::now().timestamp();
    repo.record_target_success(&t.id, Some(500)).await.unwrap();
    let s = repo.get_target(&t.id).await.unwrap().unwrap();
    assert_eq!(s.consecutive_failures, 0);
    assert!(s.last_error.is_none());
    assert_eq!(s.confirmed_watermark, Some(500));
    assert!(s.last_success_at.is_some());
    // last_attempt_at must be stamped to ~now, NOT the watermark (500). Regression
    // guard for the SQL placeholder mix-up that bound it to confirmed_watermark.
    assert!(
        s.last_attempt_at.is_some_and(|a| a >= before),
        "last_attempt_at should be now, got {:?}",
        s.last_attempt_at
    );
    assert_eq!(s.last_attempt_at, s.last_success_at, "both stamped to now");
}

#[tokio::test]
async fn watermark_set_to_supplied_value_or_left_unchanged_when_none() {
    // `record_target_success` trusts whatever watermark it's handed: the
    // contiguity/gap logic (including the deliberate exception that jumps the
    // watermark FORWARD when older items age out of the source's search
    // horizon — see `scheduler::advance_watermark`) lives entirely in the
    // caller. This repo method must not re-impose its own "only ever get
    // older" clamp on top, or it would silently swallow that unwedging move.
    let repo = repo().await;
    let t = repo.upsert_target("reddit", "search", "q").await.unwrap();
    repo.record_target_success(&t.id, Some(1000)).await.unwrap();
    assert_eq!(
        repo.get_target(&t.id)
            .await
            .unwrap()
            .unwrap()
            .confirmed_watermark,
        Some(1000)
    );
    // A newer (larger) watermark DOES overwrite — e.g. the caller unwedging
    // past an unrecoverable gap.
    repo.record_target_success(&t.id, Some(2000)).await.unwrap();
    assert_eq!(
        repo.get_target(&t.id)
            .await
            .unwrap()
            .unwrap()
            .confirmed_watermark,
        Some(2000)
    );
    // An older (smaller) one also advances it (the normal, common case).
    repo.record_target_success(&t.id, Some(500)).await.unwrap();
    assert_eq!(
        repo.get_target(&t.id)
            .await
            .unwrap()
            .unwrap()
            .confirmed_watermark,
        Some(500)
    );
    // None leaves it untouched (a head walk that didn't reach the watermark).
    repo.record_target_success(&t.id, None).await.unwrap();
    assert_eq!(
        repo.get_target(&t.id)
            .await
            .unwrap()
            .unwrap()
            .confirmed_watermark,
        Some(500)
    );
}

#[tokio::test]
async fn job_lifecycle_enqueue_progress_complete_abandon() {
    let repo = repo().await;
    let t = repo.upsert_target("reddit", "search", "q").await.unwrap();

    let job = repo.enqueue_job(&t.id, 100, 500).await.unwrap();
    assert_eq!(job.state, "open");
    assert_eq!(job.range_start, 100);
    assert_eq!(job.range_end, 500);

    // Enqueue overlapping window → coalesced to the same job (idempotent).
    let same = repo.enqueue_job(&t.id, 150, 400).await.unwrap();
    assert_eq!(same.id, job.id);
    assert_eq!(
        repo.list_open_jobs_for_target(&t.id).await.unwrap().len(),
        1
    );

    // Progress: bank cursor, bump pages, shrink range_end.
    repo.update_job_progress(&job.id, Some("t3_cur"), 1, 300)
        .await
        .unwrap();
    let p = repo.get_job(&job.id).await.unwrap().unwrap();
    assert_eq!(p.next_cursor.as_deref(), Some("t3_cur"));
    assert_eq!(p.pages_done, 1);
    assert_eq!(p.range_end, 300);
    assert_eq!(p.attempts, 1);

    // list_open_jobs_for_channel joins through the target.
    assert_eq!(
        repo.list_open_jobs_for_channel("reddit")
            .await
            .unwrap()
            .len(),
        1
    );

    repo.complete_job(&job.id).await.unwrap();
    assert_eq!(repo.get_job(&job.id).await.unwrap().unwrap().state, "done");
    assert!(repo
        .list_open_jobs_for_target(&t.id)
        .await
        .unwrap()
        .is_empty());

    // Abandon a fresh job with reason.
    let job2 = repo.enqueue_job(&t.id, 1, 50).await.unwrap();
    repo.mark_job(&job2.id, "abandoned", Some("too old"))
        .await
        .unwrap();
    let a = repo.get_job(&job2.id).await.unwrap().unwrap();
    assert_eq!(a.state, "abandoned");
    assert_eq!(a.last_error.as_deref(), Some("too old"));
}

#[tokio::test]
async fn reconcile_retires_absent_and_upsert_unretires() {
    let repo = repo().await;
    let a = repo
        .upsert_target("reddit", "search", "\"a\"")
        .await
        .unwrap();
    let b = repo
        .upsert_target("reddit", "feed", "rust+go")
        .await
        .unwrap();
    assert_eq!(repo.list_targets("reddit").await.unwrap().len(), 2);

    // Plan now contains only `a` → `b` is soft-retired (dropped from the live list
    // but the row survives).
    assert_eq!(
        repo.reconcile_targets("reddit", &[a.id.clone()])
            .await
            .unwrap(),
        1
    );
    let live = repo.list_targets("reddit").await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, a.id);

    // Re-planning `b` un-retires it via upsert (same id, status preserved).
    let b2 = repo
        .upsert_target("reddit", "feed", "rust+go")
        .await
        .unwrap();
    assert_eq!(b2.id, b.id);
    assert_eq!(repo.list_targets("reddit").await.unwrap().len(), 2);

    // An empty plan retires every live target of the channel.
    assert_eq!(repo.reconcile_targets("reddit", &[]).await.unwrap(), 2);
    assert!(repo.list_targets("reddit").await.unwrap().is_empty());
}

#[tokio::test]
async fn purge_retired_respects_cutoff() {
    let repo = repo().await;
    let t = repo.upsert_target("reddit", "search", "q").await.unwrap();
    repo.reconcile_targets("reddit", &[]).await.unwrap(); // retired_at = now
                                                          // Within the grace window (cutoff in the far past) → kept.
    assert_eq!(repo.purge_retired(0).await.unwrap(), 0);
    // Past the grace window → hard-deleted.
    assert_eq!(repo.purge_retired(i64::MAX).await.unwrap(), 1);
    assert!(
        repo.get_target(&t.id).await.unwrap().is_none(),
        "row purged"
    );
}

// ── Integration tests (through the targeted runner via "collect now") ────────

/// Helper: workspace + a single global-search reddit monitor, channel enabled.
async fn setup(app: &common::TestApp, phrase: &str) -> String {
    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "T"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap().to_string();
    app.post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id, "terms": [phrase], "channels": ["reddit"]
        }))
        .send()
        .await
        .unwrap();
    app.put("/api/channels/reddit")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();
    ws_id
}

#[tokio::test]
#[serial]
async fn normal_pass_creates_target_and_sets_status() {
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);
    let app = common::spawn_app().await;
    let ws_id = setup(&app, "testbrand").await;

    let resp = app.post("/api/admin/collect/reddit").send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // Mentions stored.
    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!mentions["items"].as_array().unwrap().is_empty());

    // A target was created with success status + watermark.
    let targets = app
        .state
        .collector_targets
        .list_targets("reddit")
        .await
        .unwrap();
    assert_eq!(targets.len(), 1, "one global-search target");
    let t = &targets[0];
    assert_eq!(t.kind, "search");
    assert_eq!(t.descriptor, "\"testbrand\"");
    assert!(t.last_success_at.is_some());
    assert_eq!(t.consecutive_failures, 0);
    assert!(t.last_error.is_none());
    assert!(
        t.confirmed_watermark.is_some(),
        "head walk reached the end → watermark set"
    );

    // The status endpoint surfaces it.
    let status: serde_json::Value = app
        .get("/api/channels/reddit/targets")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["channel"], "reddit");
    assert_eq!(status["targets"].as_array().unwrap().len(), 1);
    assert_eq!(status["targets"][0]["kind"], "search");

    // The live throttle state for the shared "reddit" lane is now surfaced. A
    // collection pass materialized the lane, so it must be present with a
    // positive rate. (In test mode the limiter is effectively unthrottled, so
    // the rate is large — just assert it exists and is > 0.)
    let throttle = &status["throttle"];
    assert!(
        throttle.is_object(),
        "throttle present after a pass: {status}"
    );
    assert!(
        throttle["rate_per_min"].as_f64().unwrap() > 0.0,
        "throttle.rate_per_min positive: {throttle}"
    );
    assert!(throttle["interval_secs"].is_number());
    assert!(throttle["paused"].is_boolean());

    std::env::remove_var("REDDIT_API_BASE");
}

#[tokio::test]
#[serial]
async fn throttling_banks_progress_then_recovers() {
    // First global search request 429s; the target records a sticky failure and
    // the pass does not lose work. A later pass retries (AIMD-paced) and recovers.
    let (reddit_base, spy) = common::mock_reddit::spawn_throttling(1).await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);
    let app = common::spawn_app().await;
    let ws_id = setup(&app, "testbrand").await;

    // Pass 1: hits the 429.
    app.post("/api/admin/collect/reddit").send().await.unwrap();

    let t = &app
        .state
        .collector_targets
        .list_targets("reddit")
        .await
        .unwrap()[0];
    assert!(t.consecutive_failures >= 1, "failure recorded");
    assert!(t.last_error.is_some(), "sticky last_error set");
    // Channel surfaces a degraded/rate-limited summary.
    let cfg: serde_json::Value = app
        .get("/api/channels/reddit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        cfg["error_message"]
            .as_str()
            .unwrap_or("")
            .contains("rate-limited"),
        "channel error_message reflects throttle: {:?}",
        cfg["error_message"]
    );

    // No mentions lost — none were ingestible on the throttled page, and the
    // 429 didn't corrupt state. There's no per-target backoff to clear: the next
    // pass simply retries (AIMD governs pacing) and recovers.
    let id = t.id.clone();

    // Pass 2: 429 budget exhausted → succeeds, clears the error, ingests.
    app.post("/api/admin/collect/reddit").send().await.unwrap();
    let t2 = app
        .state
        .collector_targets
        .get_target(&id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t2.consecutive_failures, 0, "recovered: failures cleared");
    assert!(t2.last_error.is_none(), "recovered: sticky error cleared");
    assert!(
        t2.confirmed_watermark.is_some(),
        "watermark advanced on recovery"
    );

    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !mentions["items"].as_array().unwrap().is_empty(),
        "ingested after recovery"
    );
    // The mock saw at least 2 requests (the throttled one + the recovery).
    assert!(spy.hits.load(std::sync::atomic::Ordering::SeqCst) >= 2);

    std::env::remove_var("REDDIT_API_BASE");
}

#[tokio::test]
#[serial]
async fn non_429_failure_surfaces_real_status() {
    // A non-429 upstream failure (HTTP 503) must record the real status into the
    // target's sticky last_error — not a generic "fetch failed" — and classify
    // the target as `failing` (not `throttled`).
    let (reddit_base, _spy) = common::mock_reddit::spawn_failing(3).await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);
    let app = common::spawn_app().await;
    let _ws_id = setup(&app, "testbrand").await;

    app.post("/api/admin/collect/reddit").send().await.unwrap();

    let t = &app
        .state
        .collector_targets
        .list_targets("reddit")
        .await
        .unwrap()[0];
    assert!(t.consecutive_failures >= 1, "failure recorded");
    assert_eq!(
        t.last_error.as_deref(),
        Some("HTTP 503"),
        "real status surfaced, not a generic 'fetch failed'"
    );

    // The status endpoint classifies it as failing (a non-throttle error).
    let status: serde_json::Value = app
        .get("/api/channels/reddit/targets")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["targets"][0]["health"], "failing");
    assert_eq!(status["summary"]["failing"], 1);

    std::env::remove_var("REDDIT_API_BASE");
}

#[tokio::test]
#[serial]
async fn gap_closed_by_deeper_head_paging_needs_no_backfill_job() {
    // Regression guard for the "gap detection uses page1_oldest even after
    // deeper paging" bug: the mock's `since` cutoff is wide (30 days) and the
    // watermark sits just above page 1's oldest item, so nothing stops the head
    // walk from itself paging on to page 2 within the SAME cycle — which brings
    // it back under the watermark (contiguous). Gap detection must be judged
    // against that deeper point, not page 1 alone, or it would enqueue a
    // spurious backfill job for a hole the head walk already closed.
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);
    let app = common::spawn_app().await;
    let _ws_id = setup(&app, "testbrand").await;

    // Allow a deep backfill window so the older pages aren't abandoned.
    sqlx::query("UPDATE channel_configs SET max_backfill_days = 30 WHERE channel = 'reddit'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    // Pre-seed the target with a watermark that sits BETWEEN page 1's oldest
    // item (t3_oldpost, ~10 days old) and page 2's deep item (~12 days old).
    // Page 1's oldest is then NEWER than the watermark, but page 2 (which the
    // head walk reaches on its own, within the same cycle) is OLDER than the
    // watermark — so the walk proves contiguity itself.
    let id = target_id("reddit", "search", "\"testbrand\"");
    app.state
        .collector_targets
        .upsert_target("reddit", "search", "\"testbrand\"")
        .await
        .unwrap();
    let watermark = chrono::Utc::now().timestamp() - 11 * 86_400; // ~11 days ago
    sqlx::query("UPDATE collector_targets SET confirmed_watermark = ? WHERE id = ?")
        .bind(watermark)
        .bind(&id)
        .execute(&app.state.pool)
        .await
        .unwrap();

    // One pass: the head walk pages 1 -> 2 on its own and closes the gap.
    app.post("/api/admin/collect/reddit").send().await.unwrap();

    // No backfill job was ever needed — the head walk closed the gap itself.
    let total_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM collector_backfill_jobs WHERE target_id = ?")
            .bind(&id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(
        total_jobs, 0,
        "no spurious backfill job when deeper head paging already closed the gap"
    );

    // The page-2 deep item still landed — ingested directly by the head walk.
    let exists: bool = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM mentions WHERE channel = 'reddit' AND external_id = 't3_page2deep'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
        > 0;
    assert!(exists, "head walk ingested the deep page-2 item directly");

    // The watermark advanced past the deep item (contiguity proven end to end),
    // not merely to page 1's oldest.
    let t = app
        .state
        .collector_targets
        .get_target(&id)
        .await
        .unwrap()
        .unwrap();
    let page2_deep_ts = chrono::Utc::now().timestamp() - 12 * 86_400;
    assert!(
        t.confirmed_watermark
            .is_some_and(|w| w <= page2_deep_ts + 5),
        "watermark should reach down to the page-2 item, got {:?}",
        t.confirmed_watermark
    );

    std::env::remove_var("REDDIT_API_BASE");
}

#[tokio::test]
#[serial]
async fn genuine_gap_past_full_head_walk_enqueues_backfill_and_unwedges_watermark() {
    // Regression guard for the watermark-wedging bug: seed a watermark OLDER
    // than every item the mock can ever serve (page 1's ~10 days, page 2's ~12
    // days, then a genuinely empty page 3 — nothing left to page through, the
    // Reddit-search-horizon scenario). The head walk pages all the way to that
    // empty page and STILL never reaches the watermark, so a real, irreducible
    // gap remains between the watermark and the deepest item reached.
    //
    // Without the fix, `advance_watermark` would leave the watermark pinned at
    // its old value forever (the anchor it needs can never reappear). With the
    // fix, `reached_end` lets it move forward to the deepest point actually
    // reached instead of wedging.
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);
    let app = common::spawn_app().await;
    let _ws_id = setup(&app, "testbrand").await;

    sqlx::query("UPDATE channel_configs SET max_backfill_days = 30 WHERE channel = 'reddit'")
        .execute(&app.state.pool)
        .await
        .unwrap();

    let id = target_id("reddit", "search", "\"testbrand\"");
    app.state
        .collector_targets
        .upsert_target("reddit", "search", "\"testbrand\"")
        .await
        .unwrap();
    // Older than page 2's ~12-day-old item; the mock has nothing older than
    // that (page 3 is empty), so this watermark can never be caught up to.
    let watermark = chrono::Utc::now().timestamp() - 13 * 86_400; // ~13 days ago
    sqlx::query("UPDATE collector_targets SET confirmed_watermark = ? WHERE id = ?")
        .bind(watermark)
        .bind(&id)
        .execute(&app.state.pool)
        .await
        .unwrap();

    // One pass: head walk exhausts the source (page 1, page 2, empty page 3),
    // still short of the watermark, enqueues a backfill job for the residual
    // gap, and the backfill worker drains it (also hits the empty page and
    // completes) within the same cycle.
    app.post("/api/admin/collect/reddit").send().await.unwrap();

    let open_jobs = app
        .state
        .collector_targets
        .list_open_jobs_for_target(&id)
        .await
        .unwrap();
    assert!(open_jobs.is_empty(), "backfill job drained/closed");
    let total_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM collector_backfill_jobs WHERE target_id = ?")
            .bind(&id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(
        total_jobs, 1,
        "exactly one backfill job was enqueued for the residual gap"
    );
    let done_jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM collector_backfill_jobs WHERE target_id = ? AND state IN ('done','abandoned')",
    )
    .bind(&id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(done_jobs, 1, "the gap job reached a terminal state");

    // The page-2 deep item landed regardless (head walk reached it directly).
    let exists: bool = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM mentions WHERE channel = 'reddit' AND external_id = 't3_page2deep'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap()
        > 0;
    assert!(exists, "the deep page-2 item was ingested");

    // The watermark UNWEDGED: it moved forward from the seeded ~13-days-ago
    // value to (approximately) the deepest point actually reached (~12 days),
    // rather than staying pinned at the unreachable seed forever.
    let t = app
        .state
        .collector_targets
        .get_target(&id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        t.confirmed_watermark.is_some_and(|w| w > watermark),
        "watermark must move forward past the unreachable seed instead of wedging, got {:?} (seed was {watermark})",
        t.confirmed_watermark
    );

    std::env::remove_var("REDDIT_API_BASE");
}

/// Workspace + a single global-search reddit monitor; returns (workspace_id,
/// monitor_id) so the test can edit/delete the monitor.
async fn setup_with_monitor(app: &common::TestApp, phrase: &str) -> (String, String) {
    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "T"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap().to_string();
    let mon: serde_json::Value = app
        .post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id, "terms": [phrase], "channels": ["reddit"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mon_id = mon["id"].as_str().unwrap().to_string();
    app.put("/api/channels/reddit")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();
    (ws_id, mon_id)
}

#[tokio::test]
#[serial]
async fn editing_monitor_terms_retires_the_old_target() {
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);
    let app = common::spawn_app().await;
    let (_ws, mon_id) = setup_with_monitor(&app, "testbrand").await;

    app.post("/api/admin/collect/reddit").send().await.unwrap();
    let t = app
        .state
        .collector_targets
        .list_targets("reddit")
        .await
        .unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].descriptor, "\"testbrand\"");
    // Membership edge recorded for the target's monitor.
    let edges: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM target_monitors")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(edges, 1, "target_monitors membership recorded");

    // Edit the monitor's terms → a new descriptor/target. The old target must be
    // soft-retired by reconcile, not left polling forever.
    app.put(&format!("/api/monitors/{}", mon_id))
        .json(&serde_json::json!({ "terms": ["otherword"] }))
        .send()
        .await
        .unwrap();
    app.post("/api/admin/collect/reddit").send().await.unwrap();

    let live = app
        .state
        .collector_targets
        .list_targets("reddit")
        .await
        .unwrap();
    assert_eq!(
        live.len(),
        1,
        "old target retired; only the new one is live"
    );
    assert_eq!(live[0].descriptor, "\"otherword\"");
    // The retired row still exists physically (within the grace window).
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM collector_targets WHERE channel='reddit'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert_eq!(total, 2, "old row soft-retired (kept), new row live");

    std::env::remove_var("REDDIT_API_BASE");
}

#[tokio::test]
#[serial]
async fn deleting_monitor_cascades_membership_and_retires_target() {
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);
    let app = common::spawn_app().await;
    let (_ws, mon_id) = setup_with_monitor(&app, "testbrand").await;

    app.post("/api/admin/collect/reddit").send().await.unwrap();
    assert_eq!(
        app.state
            .collector_targets
            .list_targets("reddit")
            .await
            .unwrap()
            .len(),
        1
    );

    // Deleting the monitor cascades its target_monitors edges away immediately.
    app.delete(&format!("/api/monitors/{}", mon_id))
        .send()
        .await
        .unwrap();
    let edges: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM target_monitors")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(edges, 0, "monitor delete cascaded the membership edge");

    // Next pass: the plan is empty → the orphaned target is retired (no live ones).
    app.post("/api/admin/collect/reddit").send().await.unwrap();
    assert!(
        app.state
            .collector_targets
            .list_targets("reddit")
            .await
            .unwrap()
            .is_empty(),
        "deleted monitor's target retired"
    );

    std::env::remove_var("REDDIT_API_BASE");
}
