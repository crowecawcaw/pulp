mod common;

use serial_test::serial;

// â”€â”€ HackerNews â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
#[serial]
async fn test_hackernews_backfill_filters_old_items() {
    let hn_base = common::mock_hn::spawn().await;
    std::env::set_var("HACKERNEWS_BASE_URL", &hn_base);

    let app = common::spawn_app().await;

    // Create workspace + monitor
    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "HN Backfill Test"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    app.post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id,
            "terms": ["testbrand"],
            "channels": ["hackernews"]
        }))
        .send()
        .await
        .unwrap();

    // Enable HN channel
    app.put("/api/channels/hackernews")
        .json(&serde_json::json!({"enabled": true}))
        .send()
        .await
        .unwrap();

    // Backfill with since = now - 3 days (should include 1-day-old item, exclude 10-day-old item)
    let since = chrono::Utc::now().timestamp() - 3 * 86400;
    let resp = app
        .post("/api/admin/backfill")
        .json(&serde_json::json!({ "channel": "hackernews", "since": since }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Verify only the in-window (page-1, < 3 days) items are stored; the
    // page-2 items (4 and 10 days old) are dropped by the server-side filter.
    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let items = mentions["items"].as_array().unwrap();
    assert!(
        !items.is_empty(),
        "expected in-window mentions to be stored"
    );
    for item in items {
        assert_eq!(item["channel"].as_str().unwrap(), "hackernews");
        let id = item["external_id"].as_str().unwrap();
        assert_ne!(id, "444444", "4-day-old page-2 item must be filtered out");
        assert_ne!(id, "333333", "10-day-old page-2 item must be filtered out");
    }

    std::env::remove_var("HACKERNEWS_BASE_URL");
}

// â”€â”€ Reddit â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
#[serial]
async fn test_reddit_backfill_filters_old_items() {
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);

    let app = common::spawn_app().await;

    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "Reddit Backfill Test"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    app.post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id,
            "terms": ["testbrand"],
            "channels": ["reddit"]
        }))
        .send()
        .await
        .unwrap();

    // Enable Reddit channel â€” public search needs no credentials
    app.put("/api/channels/reddit")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();

    // Backfill with since = now - 3 days. The mock returns several recent items
    // (created now) plus one item that is 10 days old. The Reddit collector does
    // client-side time filtering, so only the recent items should be stored and
    // the old one (`t3_oldpost`) must be excluded.
    let since = chrono::Utc::now().timestamp() - 3 * 86400;
    let resp = app
        .post("/api/admin/backfill")
        .json(&serde_json::json!({ "channel": "reddit", "since": since }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let items = mentions["items"].as_array().unwrap();
    assert!(!items.is_empty(), "expected recent mentions to be stored");
    for item in items {
        assert_eq!(item["channel"].as_str().unwrap(), "reddit");
        assert_ne!(
            item["external_id"].as_str().unwrap(),
            "t3_oldpost",
            "old item should have been filtered out by the backfill window"
        );
    }

    std::env::remove_var("REDDIT_API_BASE");
}

// â”€â”€ GitHub â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[tokio::test]
#[serial]
async fn test_github_backfill_filters_old_items() {
    let github_base = common::mock_github::spawn().await;
    std::env::set_var("GITHUB_BASE_URL", &github_base);

    let app = common::spawn_app().await;

    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "GitHub Backfill Test"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    app.post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id,
            "terms": ["testbrand"],
            "channels": ["github"]
        }))
        .send()
        .await
        .unwrap();

    // Enable GitHub channel with a test token
    app.put("/api/channels/github")
        .json(&serde_json::json!({
            "enabled": true,
            "credentials": {
                "token": "test_token"
            }
        }))
        .send()
        .await
        .unwrap();

    // Backfill with since = now - 3 days (server-side filter: only returns 1-day-old issue)
    let since = chrono::Utc::now().timestamp() - 3 * 86400;
    let resp = app
        .post("/api/admin/backfill")
        .json(&serde_json::json!({ "channel": "github", "since": since }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let items = mentions["items"].as_array().unwrap();
    assert!(
        !items.is_empty(),
        "expected in-window mentions to be stored"
    );
    for item in items {
        assert_eq!(item["channel"].as_str().unwrap(), "github");
        let id = item["external_id"].as_str().unwrap();
        assert_ne!(
            id, "gh_6666666",
            "4-day-old page-2 item must be filtered out"
        );
        assert_ne!(
            id, "gh_8888888",
            "10-day-old page-2 item must be filtered out"
        );
    }

    std::env::remove_var("GITHUB_BASE_URL");
}

// â”€â”€ Pagination â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//
// The mock servers serve TWO pages, page 2 strictly older than page 1, with the
// deepest item beyond the 3-day cutoff. A far-back `since` must page into page 2
// and collect those items; a recent `since` must stop at page 1.

async fn setup_app_with_monitor(
    app: &common::TestApp,
    channel: &str,
    creds: serde_json::Value,
) -> String {
    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": format!("{} pagination", channel)}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap().to_string();

    app.post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id,
            "terms": ["testbrand"],
            "channels": [channel]
        }))
        .send()
        .await
        .unwrap();

    app.put(&format!("/api/channels/{}", channel))
        .json(&serde_json::json!({ "enabled": true, "credentials": creds }))
        .send()
        .await
        .unwrap();

    ws_id
}

async fn external_ids(app: &common::TestApp, ws_id: &str) -> Vec<String> {
    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    mentions["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["external_id"].as_str().unwrap().to_string())
        .collect()
}

async fn backfill(app: &common::TestApp, channel: &str, since: i64) {
    let resp = app
        .post("/api/admin/backfill")
        .json(&serde_json::json!({ "channel": channel, "since": since }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
#[serial]
async fn test_hackernews_backfill_pages_across_multiple_pages() {
    let hn_base = common::mock_hn::spawn().await;
    std::env::set_var("HACKERNEWS_BASE_URL", &hn_base);

    let app = common::spawn_app().await;
    let ws_id = setup_app_with_monitor(&app, "hackernews", serde_json::json!({})).await;

    // Far-back backfill (30 days): must reach page 2 and collect the 4- and
    // 10-day-old items in addition to page-1 items.
    let since = chrono::Utc::now().timestamp() - 30 * 86400;
    backfill(&app, "hackernews", since).await;

    let ids = external_ids(&app, &ws_id).await;
    assert!(ids.contains(&"111111".to_string()), "page-1 recent item");
    assert!(
        ids.contains(&"444444".to_string()),
        "page-2 item must be collected via pagination, got {:?}",
        ids
    );
    assert!(
        ids.contains(&"333333".to_string()),
        "deepest page-2 item must be collected with a far-back since, got {:?}",
        ids
    );

    std::env::remove_var("HACKERNEWS_BASE_URL");
}

#[tokio::test]
#[serial]
async fn test_hackernews_recent_backfill_stays_on_page_one() {
    let hn_base = common::mock_hn::spawn().await;
    std::env::set_var("HACKERNEWS_BASE_URL", &hn_base);

    let app = common::spawn_app().await;
    let ws_id = setup_app_with_monitor(&app, "hackernews", serde_json::json!({})).await;

    // Recent `since` (3 days): server-side numericFilters drops the 4- and
    // 10-day-old page-2 items, so nothing from page 2 is collected.
    let since = chrono::Utc::now().timestamp() - 3 * 86400;
    backfill(&app, "hackernews", since).await;

    let ids = external_ids(&app, &ws_id).await;
    assert!(ids.contains(&"111111".to_string()), "page-1 recent item");
    assert!(
        !ids.contains(&"444444".to_string()) && !ids.contains(&"333333".to_string()),
        "page-2 items must NOT be collected for a recent since, got {:?}",
        ids
    );

    std::env::remove_var("HACKERNEWS_BASE_URL");
}

#[tokio::test]
#[serial]
async fn test_github_backfill_pages_across_multiple_pages() {
    let github_base = common::mock_github::spawn().await;
    std::env::set_var("GITHUB_BASE_URL", &github_base);

    let app = common::spawn_app().await;
    let ws_id =
        setup_app_with_monitor(&app, "github", serde_json::json!({ "token": "test_token" })).await;

    // Far-back backfill (30 days): pages into page 2.
    let since = chrono::Utc::now().timestamp() - 30 * 86400;
    backfill(&app, "github", since).await;

    let ids = external_ids(&app, &ws_id).await;
    assert!(
        ids.contains(&"gh_9999999".to_string()),
        "page-1 recent item"
    );
    assert!(
        ids.contains(&"gh_6666666".to_string()),
        "page-2 item must be collected via pagination, got {:?}",
        ids
    );
    assert!(
        ids.contains(&"gh_8888888".to_string()),
        "deepest page-2 item must be collected with a far-back since, got {:?}",
        ids
    );

    std::env::remove_var("GITHUB_BASE_URL");
}

#[tokio::test]
#[serial]
async fn test_github_recent_backfill_stays_on_page_one() {
    let github_base = common::mock_github::spawn().await;
    std::env::set_var("GITHUB_BASE_URL", &github_base);

    let app = common::spawn_app().await;
    let ws_id =
        setup_app_with_monitor(&app, "github", serde_json::json!({ "token": "test_token" })).await;

    // Recent `since` (3 days): the collector floors by created_at client-side, so
    // the 4-/10-day-old page-2 items are dropped. Page 1's oldest item (2 days)
    // is still in-window, so it does fetch page 2 once — but stores nothing from
    // it (its oldest item then trips the stop condition).
    let since = chrono::Utc::now().timestamp() - 3 * 86400;
    backfill(&app, "github", since).await;

    let ids = external_ids(&app, &ws_id).await;
    assert!(
        ids.contains(&"gh_9999999".to_string()),
        "page-1 recent item"
    );
    assert!(
        !ids.contains(&"gh_6666666".to_string()) && !ids.contains(&"gh_8888888".to_string()),
        "page-2 items must NOT be stored for a recent since, got {:?}",
        ids
    );

    std::env::remove_var("GITHUB_BASE_URL");
}

#[tokio::test]
#[serial]
async fn test_reddit_backfill_pages_across_multiple_pages() {
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);

    let app = common::spawn_app().await;
    let ws_id = setup_app_with_monitor(&app, "reddit", serde_json::json!({})).await;

    // Far-back backfill (30 days): page 1's oldest item (t3_oldpost, 10 days) is
    // still in-window, so the collector advances `after=t3_oldpost` to page 2 and
    // collects the ~12-day-old t3_page2deep.
    let since = chrono::Utc::now().timestamp() - 30 * 86400;
    backfill(&app, "reddit", since).await;

    let ids = external_ids(&app, &ws_id).await;
    assert!(ids.contains(&"t3_abc123".to_string()), "page-1 item");
    assert!(
        ids.contains(&"t3_oldpost".to_string()),
        "page-1 old item is in-window for a far-back since, got {:?}",
        ids
    );
    assert!(
        ids.contains(&"t3_page2deep".to_string()),
        "page-2 item must be collected via the after= cursor, got {:?}",
        ids
    );

    std::env::remove_var("REDDIT_API_BASE");
}

#[tokio::test]
#[serial]
async fn test_reddit_recent_backfill_stays_on_page_one() {
    let reddit_base = common::mock_reddit::spawn().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);

    let app = common::spawn_app().await;
    let ws_id = setup_app_with_monitor(&app, "reddit", serde_json::json!({})).await;

    // Recent `since` (3 days): page 1's oldest item (t3_oldpost, 10 days) is out
    // of window, so the collector stops after page 1 and never requests page 2.
    let since = chrono::Utc::now().timestamp() - 3 * 86400;
    backfill(&app, "reddit", since).await;

    let ids = external_ids(&app, &ws_id).await;
    assert!(ids.contains(&"t3_abc123".to_string()), "page-1 recent item");
    assert!(
        !ids.contains(&"t3_page2deep".to_string()),
        "page-2 item must NOT be collected for a recent since, got {:?}",
        ids
    );

    std::env::remove_var("REDDIT_API_BASE");
}
