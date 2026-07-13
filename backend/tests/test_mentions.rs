mod common;

use pulp::db::repos::traits::NewMention;
use serial_test::serial;

async fn setup_workspace_and_monitor(app: &common::TestApp) -> (String, String) {
    let ws_resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": "Mention WS" }))
        .send()
        .await
        .unwrap();
    let ws: serde_json::Value = ws_resp.json().await.unwrap();
    let ws_id = ws["id"].as_str().unwrap().to_string();

    let kw_resp = app
        .post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id,
            "terms": ["testbrand"]
        }))
        .send()
        .await
        .unwrap();
    let kw: serde_json::Value = kw_resp.json().await.unwrap();
    let kw_id = kw["id"].as_str().unwrap().to_string();

    (ws_id, kw_id)
}

#[tokio::test]
async fn test_list_mentions_empty() {
    let app = common::spawn_app().await;
    let ws_resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": "Empty Mention WS" }))
        .send()
        .await
        .unwrap();
    let ws: serde_json::Value = ws_resp.json().await.unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    let resp = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
    assert_eq!(body["has_more"].as_bool().unwrap(), false);
}

#[tokio::test]
async fn test_get_mention_by_id() {
    let app = common::spawn_app().await;
    let (_ws_id, kw_id) = setup_workspace_and_monitor(&app).await;

    let inserted = app
        .state
        .mentions
        .insert(NewMention {
            monitor_id: kw_id,
            channel: "hackernews".into(),
            external_id: "hn-detail-1".into(),
            content_text: "testbrand is great".into(),
            content_url: "https://news.ycombinator.com/item?id=42".into(),
            author_name: Some("alice".into()),
            author_url: None,
            published_at: Some(1000),
            platform_meta: serde_json::json!({}),
            ai_verdict: None,
        })
        .await
        .unwrap();

    // The id deep-linked from a web-push notification resolves to the mention.
    let resp = app
        .get(&format!("/api/mentions/{}", inserted.id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"].as_str().unwrap(), inserted.id);
    assert_eq!(
        body["content_url"].as_str().unwrap(),
        "https://news.ycombinator.com/item?id=42"
    );

    // An unknown id 404s rather than matching the stream/read routes.
    let missing = app
        .get("/api/mentions/does-not-exist")
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
#[serial]
async fn test_filter_by_channel() {
    let app = common::spawn_app().await;
    let (ws_id, kw_id) = setup_workspace_and_monitor(&app).await;

    // Insert two mentions with different channels using the admin collect endpoint
    // We'll use the trigger_collect approach but with different channels
    // Instead, insert directly via the mention repo through a custom approach:
    // We can use the HN collector mock to insert a hackernews mention.

    // For a simpler approach without running a collector,
    // use the repo's insert method by spawning a direct DB operation.
    // We can call the collector trigger for hackernews and github separately.

    // Spawn mock HN
    let hn_base = common::mock_hn::spawn().await;
    std::env::set_var("HACKERNEWS_BASE_URL", &hn_base);

    // Enable hackernews channel
    app.put("/api/channels/hackernews")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();

    // Trigger HN collect
    app.post("/api/admin/collect/hackernews")
        .send()
        .await
        .unwrap();

    std::env::remove_var("HACKERNEWS_BASE_URL");

    // Filter by hackernews channel
    let resp = app
        .get(&format!(
            "/api/mentions?workspace_id={}&channel=hackernews",
            ws_id
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    // All returned items should be hackernews channel
    for item in items {
        assert_eq!(item["channel"].as_str().unwrap(), "hackernews");
    }

    // Filter by reddit - should be empty (we didn't collect reddit)
    let resp2 = app
        .get(&format!(
            "/api/mentions?workspace_id={}&channel=reddit",
            ws_id
        ))
        .send()
        .await
        .unwrap();
    let body2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(body2["items"].as_array().unwrap().len(), 0);

    let _ = kw_id; // used indirectly via collect
}

async fn insert_mention(
    app: &common::TestApp,
    monitor_id: &str,
    external_id: &str,
    published_at: i64,
) {
    insert_mention_opt(app, monitor_id, external_id, Some(published_at)).await;
}

async fn insert_mention_opt(
    app: &common::TestApp,
    monitor_id: &str,
    external_id: &str,
    published_at: Option<i64>,
) {
    app.state
        .mentions
        .insert(NewMention {
            monitor_id: monitor_id.to_string(),
            channel: "reddit".to_string(),
            external_id: external_id.to_string(),
            content_text: format!("content {external_id}"),
            content_url: format!("https://example.com/{external_id}"),
            author_name: None,
            author_url: None,
            published_at,
            platform_meta: serde_json::json!({}),
            ai_verdict: None,
        })
        .await
        .unwrap();
}

// Regression: the feed's "last N days" filter must use `since` (a lower bound
// on published_at), not `before` (the keyset cursor / upper bound). Using the
// wrong one inverted the range and showed only mentions OLDER than the window.
#[tokio::test]
async fn test_since_and_before_bound_published_at() {
    let app = common::spawn_app().await;
    let (ws_id, kw_id) = setup_workspace_and_monitor(&app).await;

    let old = 1_700_000_000; // ~2023-11
    let recent = 1_750_000_000; // ~2025-06
    insert_mention(&app, &kw_id, "old-1", old).await;
    insert_mention(&app, &kw_id, "recent-1", recent).await;

    let cutoff = (old + recent) / 2;

    // since => only the recent mention (published_at >= cutoff)
    let resp = app
        .get(&format!(
            "/api/mentions?workspace_id={ws_id}&since={cutoff}"
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "since should return only the recent mention"
    );
    assert_eq!(items[0]["external_id"], "recent-1");

    // before => only the old mention (published_at < cutoff)
    let resp = app
        .get(&format!(
            "/api/mentions?workspace_id={ws_id}&before={cutoff}"
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "before should return only the old mention");
    assert_eq!(items[0]["external_id"], "old-1");

    // since + before together bracket an (empty here) window
    let resp = app
        .get(&format!(
            "/api/mentions?workspace_id={ws_id}&since={}&before={}",
            cutoff, cutoff
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

// Regression: SQLite treats a negative LIMIT as "unlimited" (no clause at
// all), so `?limit=-1` used to dump the entire table regardless of how many
// mentions existed. `MentionRepo::list` now clamps `filter.limit` to
// `[1, 200]`, so a negative limit must behave like the smallest valid page
// (1), not "everything".
#[tokio::test]
async fn test_negative_limit_is_clamped_not_unbounded() {
    let app = common::spawn_app().await;
    let (ws_id, kw_id) = setup_workspace_and_monitor(&app).await;

    insert_mention(&app, &kw_id, "nimbus-1", 1_700_000_001).await;
    insert_mention(&app, &kw_id, "nimbus-2", 1_700_000_002).await;
    insert_mention(&app, &kw_id, "nimbus-3", 1_700_000_003).await;

    let resp = app
        .get(&format!("/api/mentions?workspace_id={ws_id}&limit=-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "limit=-1 must clamp to the minimum page size (1), not return all 3 rows"
    );
    assert!(
        body["has_more"].as_bool().unwrap(),
        "3 mentions exist but only 1 was returned, so more must remain"
    );
}

// Regression: the feed Monitor dropdown sends `monitor_id`, which the API must
// honor (previously it was dropped, so the dropdown did nothing).
#[tokio::test]
async fn test_filter_by_monitor_id() {
    let app = common::spawn_app().await;
    let (ws_id, kw_a) = setup_workspace_and_monitor(&app).await;

    let kw_b_resp = app
        .post("/api/monitors")
        .json(&serde_json::json!({ "workspace_id": ws_id, "terms": ["other"] }))
        .send()
        .await
        .unwrap();
    let kw_b: serde_json::Value = kw_b_resp.json().await.unwrap();
    let kw_b = kw_b["id"].as_str().unwrap().to_string();

    insert_mention(&app, &kw_a, "a-1", 1_750_000_000).await;
    insert_mention(&app, &kw_b, "b-1", 1_750_000_001).await;

    let resp = app
        .get(&format!(
            "/api/mentions?workspace_id={ws_id}&monitor_id={kw_a}"
        ))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["external_id"], "a-1");
}

// Feed mentions can be marked read/unread and filtered by read state.
#[tokio::test]
async fn test_mark_read_and_filter() {
    let app = common::spawn_app().await;
    let (ws_id, kw_id) = setup_workspace_and_monitor(&app).await;

    insert_mention(&app, &kw_id, "m-1", 1_750_000_000).await;
    insert_mention(&app, &kw_id, "m-2", 1_750_000_001).await;

    // Grab one mention's id.
    let resp = app
        .get(&format!("/api/mentions?workspace_id={ws_id}"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    // Every mention starts unread.
    assert!(items.iter().all(|m| m["read_at"].is_null()));
    let target = items[0]["id"].as_str().unwrap().to_string();

    // Mark it read.
    let resp = app
        .put(&format!("/api/mentions/{target}/read"))
        .json(&serde_json::json!({ "read": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let updated: serde_json::Value = resp.json().await.unwrap();
    assert!(updated["read_at"].as_i64().is_some());

    // read=false → only the still-unread mention.
    let resp = app
        .get(&format!("/api/mentions?workspace_id={ws_id}&read=false"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_ne!(items[0]["id"].as_str().unwrap(), target);

    // read=true → only the one we marked.
    let resp = app
        .get(&format!("/api/mentions?workspace_id={ws_id}&read=true"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"].as_str().unwrap(), target);

    // Mark it unread again → back to 2 unread.
    app.put(&format!("/api/mentions/{target}/read"))
        .json(&serde_json::json!({ "read": false }))
        .send()
        .await
        .unwrap();
    let resp = app
        .get(&format!("/api/mentions?workspace_id={ws_id}&read=false"))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_mark_read_unknown_id_404() {
    let app = common::spawn_app().await;
    let resp = app
        .put("/api/mentions/does-not-exist/read")
        .json(&serde_json::json!({ "read": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
#[serial]
async fn test_list_mentions_pagination() {
    let app = common::spawn_app().await;
    let (ws_id, _kw_id) = setup_workspace_and_monitor(&app).await;

    // The HN mock's fixture text embeds the query term, so every hit it
    // returns matches the "testbrand" monitor and gets ingested — the mock
    // yields more than one page's worth (well past `limit=1` below) once the
    // default 7-day backfill window pages past page 1.
    let hn_base = common::mock_hn::spawn().await;
    std::env::set_var("HACKERNEWS_BASE_URL", &hn_base);

    app.put("/api/channels/hackernews")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();

    app.post("/api/admin/collect/hackernews")
        .send()
        .await
        .unwrap();

    std::env::remove_var("HACKERNEWS_BASE_URL");

    // Discover the TRUE total (well above the clamp floor) so the assertion
    // below doesn't have to guess/guard on how many mentions the mock
    // produced — it was previously wrapped in `if items.len() == 1 { .. }`,
    // which let the test pass while asserting nothing at all if that branch
    // was never taken.
    let all_resp = app
        .get(&format!("/api/mentions?workspace_id={}&limit=200", ws_id))
        .send()
        .await
        .unwrap();
    let all_body: serde_json::Value = all_resp.json().await.unwrap();
    let total = all_body["items"].as_array().unwrap().len();
    assert!(
        total >= 2,
        "expected at least 2 mentions from the HN mock, got {total}"
    );

    // Mock returns multiple mentions; request limit=1 to test pagination.
    let resp = app
        .get(&format!("/api/mentions?workspace_id={}&limit=1", ws_id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "limit=1 must return exactly 1 item");
    // With more mentions than the page size, has_more must unconditionally
    // be true — this is the assertion the old guarded version could skip.
    assert!(
        body["has_more"].as_bool().unwrap(),
        "has_more must be true when {total} mentions exist and limit=1"
    );
}

// Regression: the old cursor was a single `published_at < before` filter.
// `published_at` is nullable and non-unique, so that scheme (a) dropped rows
// with a NULL `published_at` from every cursor page (only page 1, via
// unfiltered `ORDER BY published_at DESC`, ever showed them) and (b) skipped
// or duplicated rows that tied on `published_at` across a page boundary,
// because "strictly before the last row's timestamp" silently excludes any
// sibling that shares it.
//
// The fix orders by `COALESCE(published_at, ingested_at) DESC, id DESC` and
// takes a compound `(before, before_id)` cursor, so walking the feed one row
// at a time (`limit=1`) must visit every row exactly once, in exactly the
// order the unpaginated listing (`limit=200`) reports — Nimbus/Fern mentions
// mixing tied `published_at` values and a NULL one.
#[tokio::test]
async fn test_pagination_compound_cursor_handles_ties_and_null_published_at() {
    let app = common::spawn_app().await;
    let (ws_id, kw_id) = setup_workspace_and_monitor(&app).await;

    // Three mentions that TIE on published_at (a naive `before` cursor skips
    // or duplicates across this tie when paging one at a time).
    let tie_ts = 1_700_000_000; // ~2023-11, Nimbus benchmark thread
    insert_mention(&app, &kw_id, "nimbus-hn-bench-a", tie_ts).await;
    insert_mention(&app, &kw_id, "nimbus-hn-bench-b", tie_ts).await;
    insert_mention(&app, &kw_id, "nimbus-hn-bench-c", tie_ts).await;

    // One mention with published_at = NULL (e.g. a GitHub issue comment whose
    // platform timestamp didn't parse). Its effective timestamp is its
    // ingested_at, which — being "now" — sorts ahead of the tie_ts trio.
    insert_mention_opt(&app, &kw_id, "fern-gh-issue-null-ts", None).await;

    // Ground truth: the unpaginated listing (large enough limit to get
    // everything in one shot) establishes the canonical order.
    let all_resp = app
        .get(&format!("/api/mentions?workspace_id={ws_id}&limit=200"))
        .send()
        .await
        .unwrap();
    let all_body: serde_json::Value = all_resp.json().await.unwrap();
    let all_items = all_body["items"].as_array().unwrap();
    assert_eq!(
        all_items.len(),
        4,
        "all 4 mentions must be listed, including the NULL-published_at one"
    );
    let expected_ids: Vec<String> = all_items
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    // The NULL-published_at row has the largest effective timestamp
    // (ingested_at ~ now >> tie_ts), so it must sort first.
    assert_eq!(
        all_items[0]["external_id"].as_str().unwrap(),
        "fern-gh-issue-null-ts",
        "the NULL published_at row must still appear, ordered by its ingested_at"
    );

    // Now walk the feed one row at a time using the compound cursor, exactly
    // as the frontend's "load more" does: each step's cursor is the last
    // item's (published_at ?? ingested_at, id).
    let mut paged_ids: Vec<String> = Vec::new();
    let mut before: Option<i64> = None;
    let mut before_id: Option<String> = None;
    loop {
        let url = match (before, &before_id) {
            (Some(b), Some(bid)) => {
                format!("/api/mentions?workspace_id={ws_id}&limit=1&before={b}&before_id={bid}")
            }
            _ => format!("/api/mentions?workspace_id={ws_id}&limit=1"),
        };
        let resp = app.get(&url).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        let items = body["items"].as_array().unwrap();
        assert_eq!(
            items.len(),
            1,
            "limit=1 must return exactly one row per page"
        );
        let item = &items[0];
        let id = item["id"].as_str().unwrap().to_string();
        paged_ids.push(id.clone());

        let eff_ts = item["published_at"]
            .as_i64()
            .unwrap_or_else(|| item["ingested_at"].as_i64().unwrap());
        before = Some(eff_ts);
        before_id = Some(id);

        if !body["has_more"].as_bool().unwrap() {
            break;
        }

        // Guard against an infinite loop if has_more is ever wrong.
        assert!(
            paged_ids.len() <= expected_ids.len(),
            "paged past the total mention count without has_more going false"
        );
    }

    assert_eq!(
        paged_ids, expected_ids,
        "one-row-at-a-time pagination via the compound cursor must visit every mention \
         exactly once, in the same order as the unpaginated listing — no skips or \
         duplicates across the tied published_at values, and the NULL-published_at row \
         must not vanish"
    );
}
