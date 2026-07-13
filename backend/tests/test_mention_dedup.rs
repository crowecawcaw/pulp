//! Per-monitor dedup: `mentions.UNIQUE(monitor_id, channel, external_id)` (not
//! `UNIQUE(channel, external_id)`) means the same external post is stored once
//! PER MATCHING MONITOR, so a post matching two monitors — even across
//! different workspaces — must be ingested for both, each with its own read
//! state / AI verdict / notified_at. Regression coverage for the gap where a
//! global `(channel, external_id)` constraint let only the first monitor to
//! see a post ever store it.

mod common;

use pulp::db::repos::traits::NewMention;
use serial_test::serial;

async fn create_workspace(app: &common::TestApp, name: &str) -> String {
    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    ws["id"].as_str().unwrap().to_string()
}

/// Create a monitor with the given term/channel and return its id.
async fn create_monitor(app: &common::TestApp, ws_id: &str, term: &str, channel: &str) -> String {
    let m: serde_json::Value = app
        .post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id,
            "terms": [term],
            "channels": [channel]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    m["id"].as_str().unwrap().to_string()
}

/// Every mention's `external_id` for a workspace's feed, as a sorted vec (so
/// duplicates and set contents are both easy to assert on).
async fn feed_external_ids(app: &common::TestApp, ws_id: &str) -> Vec<String> {
    let body: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_id}&limit=200"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut ids: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["external_id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

// -- Targeted collection path (scheduler::store_page_mentions, reddit) ------

/// Reddit is the "targeted" collector: monitors sharing a channel are
/// OR-batched into one upstream search and the resulting page is fanned out
/// per-monitor by `scheduler::store_page_mentions`. Two monitors in two
/// different workspaces that both match the mock's fixture posts must each
/// get their own mention rows for the same external posts.
#[tokio::test]
#[serial]
async fn same_reddit_mention_lands_in_both_workspaces_via_target_path() {
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);

    let app = common::spawn_app().await;
    let ws_a = create_workspace(&app, "A").await;
    let ws_b = create_workspace(&app, "B").await;
    let _mon_a = create_monitor(&app, &ws_a, "testbrand", "reddit").await;
    let _mon_b = create_monitor(&app, &ws_b, "testbrand", "reddit").await;

    app.put("/api/channels/reddit")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();

    let resp = app.post("/api/admin/collect/reddit").send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let ids_a = feed_external_ids(&app, &ws_a).await;
    let ids_b = feed_external_ids(&app, &ws_b).await;

    assert!(!ids_a.is_empty(), "workspace A should have mentions");
    assert_eq!(
        ids_a, ids_b,
        "both workspaces' monitors matched the same posts, so both feeds \
         must contain the same set of external ids — the second monitor \
         must no longer be starved by the first monitor's insert"
    );

    // Confirm they are genuinely separate rows (different mention ids), not
    // one row visible through both queries.
    let a_body: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_a}&limit=200"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b_body: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_b}&limit=200"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let a_mention_ids: std::collections::HashSet<String> = a_body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    let b_mention_ids: std::collections::HashSet<String> = b_body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        a_mention_ids.is_disjoint(&b_mention_ids),
        "each workspace must own its own mention row, not share the same id"
    );

    std::env::remove_var("REDDIT_API_BASE");
}

/// Idempotency: re-running the same collection pass must not duplicate rows
/// for a monitor that already has them (the `exists` check, now scoped to
/// `(monitor_id, channel, external_id)`, still gates re-insertion).
#[tokio::test]
#[serial]
async fn collecting_reddit_twice_does_not_duplicate_per_monitor() {
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);

    let app = common::spawn_app().await;
    let ws_a = create_workspace(&app, "A").await;
    let ws_b = create_workspace(&app, "B").await;
    let _mon_a = create_monitor(&app, &ws_a, "testbrand", "reddit").await;
    let _mon_b = create_monitor(&app, &ws_b, "testbrand", "reddit").await;

    app.put("/api/channels/reddit")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();

    app.post("/api/admin/collect/reddit").send().await.unwrap();
    let ids_a_first = feed_external_ids(&app, &ws_a).await;
    let ids_b_first = feed_external_ids(&app, &ws_b).await;
    assert!(!ids_a_first.is_empty());

    // Second pass over the same (unchanged) mock dataset.
    app.post("/api/admin/collect/reddit").send().await.unwrap();
    let ids_a_second = feed_external_ids(&app, &ws_a).await;
    let ids_b_second = feed_external_ids(&app, &ws_b).await;

    assert_eq!(
        ids_a_first, ids_a_second,
        "re-collecting must not duplicate workspace A's mentions"
    );
    assert_eq!(
        ids_b_first, ids_b_second,
        "re-collecting must not duplicate workspace B's mentions"
    );

    std::env::remove_var("REDDIT_API_BASE");
}

// -- Legacy (non-targeted) collection path (collectors::mod, hackernews) ----

/// Hacker News is a non-targeted collector: `run_pass` in `collectors/mod.rs`
/// calls `fetch_pass` per relevant monitor directly (no `scheduler.rs`
/// involved). The mock returns the same fixture hits regardless of the query
/// text, so two monitors — in different workspaces — both "match" and must
/// both get their own copies.
#[tokio::test]
#[serial]
async fn same_hn_mention_lands_in_both_workspaces_via_legacy_path() {
    let hn_base = common::mock_hn::spawn().await;
    std::env::set_var("HACKERNEWS_BASE_URL", &hn_base);

    let app = common::spawn_app().await;
    let ws_a = create_workspace(&app, "A").await;
    let ws_b = create_workspace(&app, "B").await;
    let _mon_a = create_monitor(&app, &ws_a, "testbrand", "hackernews").await;
    let _mon_b = create_monitor(&app, &ws_b, "testbrand", "hackernews").await;

    app.put("/api/channels/hackernews")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();

    let resp = app
        .post("/api/admin/collect/hackernews")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let ids_a = feed_external_ids(&app, &ws_a).await;
    let ids_b = feed_external_ids(&app, &ws_b).await;

    assert!(!ids_a.is_empty(), "workspace A should have mentions");
    assert_eq!(
        ids_a, ids_b,
        "both workspaces' monitors matched the same HN hits, so both feeds \
         must contain the same set of external ids"
    );

    std::env::remove_var("HACKERNEWS_BASE_URL");
}

// -- Repo-level: exists()/insert() semantics --------------------------------

/// Direct repo-level check that `MentionRepo::exists` is scoped per monitor,
/// and that `insert` allows the same `(channel, external_id)` under a
/// different `monitor_id` — while a true re-insert for the SAME monitor still
/// hits the `UNIQUE(monitor_id, channel, external_id)` constraint.
#[tokio::test]
async fn exists_and_insert_are_scoped_per_monitor() {
    let app = common::spawn_app().await;
    let ws_a = create_workspace(&app, "A").await;
    let ws_b = create_workspace(&app, "B").await;
    let mon_a = create_monitor(&app, &ws_a, "testbrand", "reddit").await;
    let mon_b = create_monitor(&app, &ws_b, "testbrand", "reddit").await;

    let new_mention = |monitor_id: &str| NewMention {
        monitor_id: monitor_id.to_string(),
        channel: "reddit".to_string(),
        external_id: "t3_shared".to_string(),
        content_text: "shared post".to_string(),
        content_url: "https://reddit.com/t3_shared".to_string(),
        author_name: None,
        author_url: None,
        published_at: Some(1_700_000_000),
        platform_meta: serde_json::json!({}),
        ai_verdict: None,
    };

    // Nothing stored yet for either monitor.
    assert!(!app
        .state
        .mentions
        .exists(&mon_a, "reddit", "t3_shared")
        .await
        .unwrap());
    assert!(!app
        .state
        .mentions
        .exists(&mon_b, "reddit", "t3_shared")
        .await
        .unwrap());

    // Insert for monitor A.
    app.state
        .mentions
        .insert(new_mention(&mon_a))
        .await
        .unwrap();

    assert!(
        app.state
            .mentions
            .exists(&mon_a, "reddit", "t3_shared")
            .await
            .unwrap(),
        "exists() must see monitor A's own row"
    );
    assert!(
        !app.state
            .mentions
            .exists(&mon_b, "reddit", "t3_shared")
            .await
            .unwrap(),
        "exists() must NOT be satisfied by another monitor's row for the same \
         (channel, external_id) — dedup is per-monitor, not global"
    );

    // The same (channel, external_id) under a DIFFERENT monitor must be
    // insertable (this is the whole point of the fix).
    let inserted_b = app.state.mentions.insert(new_mention(&mon_b)).await;
    assert!(
        inserted_b.is_ok(),
        "inserting the same external post for a different monitor must succeed, got {:?}",
        inserted_b.err()
    );
    assert!(app
        .state
        .mentions
        .exists(&mon_b, "reddit", "t3_shared")
        .await
        .unwrap());

    // Re-inserting for the SAME monitor still violates the (now per-monitor)
    // unique constraint — idempotency is preserved.
    let dup = app.state.mentions.insert(new_mention(&mon_a)).await;
    assert!(
        dup.is_err(),
        "re-inserting the same (monitor_id, channel, external_id) must still fail"
    );
}

// -- AI gating: per-monitor, not global --------------------------------------

/// One monitor has an AI filter prompt, the other doesn't; both match the
/// same external post. The gated monitor's copy must be withheld
/// (`ai_verdict = 'pending'`) while the ungated monitor's copy is immediately
/// feed-visible — proving AI gating state, like everything else, is scoped
/// per mention row / per monitor rather than shared globally by
/// `(channel, external_id)`.
#[tokio::test]
async fn ai_gating_is_independent_per_monitor_for_the_same_post() {
    let app = common::spawn_app().await;
    let ws_gated = create_workspace(&app, "Gated").await;
    let ws_open = create_workspace(&app, "Open").await;

    let gated_monitor: serde_json::Value = app
        .post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_gated,
            "terms": ["testbrand"],
            "channels": ["reddit"],
            "ai_filter_prompt": "genuine product discussion"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let gated_monitor_id = gated_monitor["id"].as_str().unwrap();

    let open_monitor_id = create_monitor(&app, &ws_open, "testbrand", "reddit").await;

    // Same external post, inserted once per monitor with the appropriate
    // `ai_verdict` — mirroring what `run_pass`/`store_page_mentions` would do
    // (pending for the AI-gated monitor, None for the ungated one).
    app.state
        .mentions
        .insert(NewMention {
            monitor_id: gated_monitor_id.to_string(),
            channel: "reddit".to_string(),
            external_id: "t3_shared".to_string(),
            content_text: "shared post".to_string(),
            content_url: "https://reddit.com/t3_shared".to_string(),
            author_name: None,
            author_url: None,
            published_at: Some(1_700_000_000),
            platform_meta: serde_json::json!({}),
            ai_verdict: Some("pending".to_string()),
        })
        .await
        .unwrap();
    app.state
        .mentions
        .insert(NewMention {
            monitor_id: open_monitor_id.clone(),
            channel: "reddit".to_string(),
            external_id: "t3_shared".to_string(),
            content_text: "shared post".to_string(),
            content_url: "https://reddit.com/t3_shared".to_string(),
            author_name: None,
            author_url: None,
            published_at: Some(1_700_000_000),
            platform_meta: serde_json::json!({}),
            ai_verdict: None,
        })
        .await
        .unwrap();

    // The gated workspace's feed hides its pending copy by default.
    let gated_feed: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_gated}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        gated_feed["items"].as_array().unwrap().len(),
        0,
        "the AI-gated monitor's copy must stay pending/hidden"
    );

    // The ungated workspace's feed shows its copy immediately.
    let open_feed: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_open}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = open_feed["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "the ungated monitor's copy must be feed-visible right away"
    );
    assert_eq!(items[0]["external_id"], "t3_shared");
    assert!(items[0]["ai_verdict"].is_null());

    // The gated workspace's `ai=pending` view still finds its own copy.
    let gated_pending: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_gated}&ai=pending"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pending_items = gated_pending["items"].as_array().unwrap();
    assert_eq!(pending_items.len(), 1);
    assert_eq!(pending_items[0]["ai_verdict"], "pending");
}
