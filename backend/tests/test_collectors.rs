mod common;

use httpmock::prelude::*;
use serial_test::serial;

/// Regression test: a collector hitting HTTP 429 must abort the rest of the
/// pass instead of hammering an already-throttled API. `Collector::fetch_pass`
/// has always documented this ("stopping early when a fetch reports
/// RateLimited"), but no collector ever actually constructed a `RateLimited`
/// error, so the early-stop branch was dead code — every monitor's request
/// still went out even after a 429. GitHub's default `fetch_pass` issues one
/// HTTP request per monitor, so with two monitors and a mock that always
/// 429s, the fixed behavior must stop after exactly ONE request.
#[tokio::test]
#[serial]
async fn test_rate_limit_aborts_pass_for_remaining_monitors() {
    let mock_server = MockServer::start();
    let m = mock_server.mock(|when, then| {
        when.method(GET).path("/search/issues");
        then.status(429);
    });
    std::env::set_var("GITHUB_BASE_URL", mock_server.url(""));

    let app = common::spawn_app().await;

    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "GitHub Rate Limit Test"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    // Two monitors on the same channel — the default `fetch_pass` issues one
    // request per monitor, so this is what lets us observe the abort.
    for term in ["nimbusdb", "fernlint"] {
        app.post("/api/monitors")
            .json(&serde_json::json!({
                "workspace_id": ws_id,
                "terms": [term],
                "channels": ["github"]
            }))
            .send()
            .await
            .unwrap();
    }

    app.put("/api/channels/github")
        .json(&serde_json::json!({
            "enabled": true,
            "credentials": { "token": "test_token" }
        }))
        .send()
        .await
        .unwrap();

    let resp = app.post("/api/admin/collect/github").send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // Only the FIRST monitor's request went out — the pass aborted on the
    // 429 instead of also hitting the second monitor's request.
    m.assert_hits(1);

    // The channel's error_message reflects the rate limit, not a generic
    // "GitHub API returned status 429" (proving the RateLimited path, not the
    // fallback bail!, was taken).
    let channel: serde_json::Value = app
        .get("/api/channels/github")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let err = channel["error_message"].as_str().unwrap_or_default();
    assert!(
        err.contains("rate limited") || err.contains("429"),
        "expected a rate-limit error message, got: {err:?}"
    );

    std::env::remove_var("GITHUB_BASE_URL");
}

#[tokio::test]
#[serial]
async fn test_hackernews_collector_stores_mentions() {
    // Spawn mock HN server
    let hn_base = common::mock_hn::spawn().await;

    // Set env var so HN collector hits our mock
    std::env::set_var("HACKERNEWS_BASE_URL", &hn_base);

    // Spawn app
    let app = common::spawn_app().await;

    // Create workspace + monitor using the phrase that the mock will echo back
    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "HN Test"}))
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

    // Trigger one collection cycle
    let resp = app
        .post("/api/admin/collect/hackernews")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Verify mentions were stored
    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let items = mentions["items"].as_array().unwrap();
    assert!(!items.is_empty());
    let first = &items[0];
    assert_eq!(first["channel"].as_str().unwrap(), "hackernews");

    for m in items {
        // Every HN mention links to the HN discussion page, not the external
        // article; the article URL is kept in platform_meta.story_url.
        let url = m["content_url"].as_str().unwrap();
        assert!(
            url.starts_with("https://news.ycombinator.com/item?id="),
            "HN mention should link to the item page, got {url}"
        );
        // HTML is stripped at ingest — no raw tags or undecoded entities.
        let text = m["content_text"].as_str().unwrap();
        assert!(
            !text.contains('<') && !text.contains("&amp;") && !text.contains("&#"),
            "HN content should be HTML-stripped, got {text:?}"
        );
        // kind signals post vs comment.
        let kind = m["platform_meta"]["kind"].as_str().unwrap();
        assert!(kind == "story" || kind == "comment", "kind: {kind}");
    }

    // The comment hit carries its parent article's title (so the UI can show
    // what a comment is about) and the article URL.
    let comment = items
        .iter()
        .find(|m| m["platform_meta"]["kind"] == "comment")
        .expect("a comment mention");
    assert!(comment["platform_meta"]["title"]
        .as_str()
        .unwrap()
        .starts_with("Ask HN:"));
    assert!(comment["platform_meta"]["story_url"]
        .as_str()
        .unwrap()
        .starts_with("https://example.com/article/"));

    // Cleanup env var
    std::env::remove_var("HACKERNEWS_BASE_URL");
}

#[tokio::test]
#[serial]
async fn test_hackernews_multi_term_monitor_ors_terms_via_separate_requests() {
    // Regression test: HN's Algolia search has no boolean OR operator (only
    // AND-of-all-words by default), so a multi-term monitor used to be
    // joined into ONE space-separated query — under-recalling to only hits
    // containing every term. The fix issues one search PER term instead.
    let (hn_base, spy) = common::mock_hn::spawn_counted().await;
    std::env::set_var("HACKERNEWS_BASE_URL", &hn_base);

    let app = common::spawn_app().await;

    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "HN OR Test"}))
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
            "terms": ["nimbusdb", "fernlint"],
            "channels": ["hackernews"]
        }))
        .send()
        .await
        .unwrap();

    app.put("/api/channels/hackernews")
        .json(&serde_json::json!({"enabled": true}))
        .send()
        .await
        .unwrap();

    let resp = app
        .post("/api/admin/collect/hackernews")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Two distinct, unjoined per-term queries were sent — never the old
    // space-joined "nimbusdb fernlint" form, which Algolia would have
    // required both words to appear in the same hit.
    let queries: std::collections::HashSet<String> =
        spy.queries.lock().unwrap().iter().cloned().collect();
    assert!(queries.contains("nimbusdb"), "queries: {queries:?}");
    assert!(queries.contains("fernlint"), "queries: {queries:?}");
    assert!(
        !queries.contains("nimbusdb fernlint"),
        "terms must not be joined into one query: {queries:?}"
    );

    std::env::remove_var("HACKERNEWS_BASE_URL");
}

#[tokio::test]
#[serial]
async fn test_reddit_collector_stores_mentions() {
    // Spawn mock Reddit server (public, unauthenticated search endpoint)
    let reddit_base = common::mock_reddit::spawn().await;

    std::env::set_var("REDDIT_API_BASE", &reddit_base);

    let app = common::spawn_app().await;

    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "Reddit Test"}))
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

    // Enable reddit channel â€” no credentials needed for public search
    app.put("/api/channels/reddit")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();

    // Trigger one collection cycle
    let resp = app.post("/api/admin/collect/reddit").send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // Verify mentions were stored
    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(!mentions["items"].as_array().unwrap().is_empty());
    let first = &mentions["items"][0];
    assert_eq!(first["channel"].as_str().unwrap(), "reddit");

    std::env::remove_var("REDDIT_API_BASE");
}

#[tokio::test]
#[serial]
async fn test_reddit_global_monitors_share_one_or_batched_search() {
    let (reddit_base, spy) = common::mock_reddit::spawn_counted().await;
    std::env::set_var("REDDIT_API_BASE", &reddit_base);

    let app = common::spawn_app().await;

    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "Reddit OR-batch Test"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    // TWO unscoped (global-search) reddit monitors.
    for phrase in ["testbrand", "qualybrand"] {
        app.post("/api/monitors")
            .json(&serde_json::json!({
                "workspace_id": ws_id,
                "terms": [phrase],
                "channels": ["reddit"]
            }))
            .send()
            .await
            .unwrap();
    }

    app.put("/api/channels/reddit")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();

    let resp = app.post("/api/admin/collect/reddit").send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // Both monitors were served by ONE search request (the mock stops paging
    // after page 1 for a recent `since`), with both phrases OR-ed and quoted.
    // Terms are canonically sorted (so term order can't churn the target id),
    // hence "qualybrand" precedes "testbrand".
    assert_eq!(spy.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    let q = spy.last_q.lock().unwrap().clone();
    assert_eq!(q, r#""qualybrand" OR "testbrand""#);

    // And mentions landed (the mock echoes the query into entry text, so the
    // entries match).
    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!mentions["items"].as_array().unwrap().is_empty());

    std::env::remove_var("REDDIT_API_BASE");
}

#[tokio::test]
#[serial]
async fn test_github_collector_stores_mentions() {
    let github_base = common::mock_github::spawn().await;
    std::env::set_var("GITHUB_BASE_URL", &github_base);

    let app = common::spawn_app().await;

    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "GitHub Test"}))
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

    // Enable github channel with a test token
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

    // Trigger one collection cycle
    let resp = app.post("/api/admin/collect/github").send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // Verify mentions were stored
    let mentions: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(!mentions["items"].as_array().unwrap().is_empty());
    let first = &mentions["items"][0];
    assert_eq!(first["channel"].as_str().unwrap(), "github");

    std::env::remove_var("GITHUB_BASE_URL");
}
