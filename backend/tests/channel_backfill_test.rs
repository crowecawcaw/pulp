mod common;

use httpmock::prelude::*;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn backfill_fetches_from_github_and_inserts_mentions() {
    let mock_server = MockServer::start();

    let _m = mock_server.mock(|when, then| {
        when.method(GET).path("/search/issues");
        then.status(200).json_body(serde_json::json!({
            "items": [{
                "id": 99001,
                "title": "fern build error",
                "body": "This is about a Fern build failure",
                "html_url": "https://github.com/external/lib/issues/5",
                "user": { "login": "devuser", "html_url": "https://github.com/devuser" },
                "created_at": chrono::Utc::now().to_rfc3339(),
                "state": "open",
                "repository_url": "https://api.github.com/repos/external/lib"
            }]
        }));
    });

    // Point collector at mock server
    std::env::set_var("GITHUB_BASE_URL", mock_server.url(""));

    let app = common::spawn_app().await;

    // Create workspace + monitor
    let ws_resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "test-backfill"}))
        .send()
        .await
        .unwrap();
    let ws: serde_json::Value = ws_resp.json().await.unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    app.post("/api/monitors")
        .json(&serde_json::json!({"workspace_id": ws_id, "terms": ["fern"]}))
        .send()
        .await
        .unwrap();

    // Enable github channel
    app.put("/api/channels/github")
        .json(&serde_json::json!({"enabled": true, "credentials": {"token": ""}}))
        .send()
        .await
        .unwrap();

    // Trigger backfill
    let resp = app
        .post("/api/channels/github/backfill")
        .json(&serde_json::json!({"days": 7}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["message"].as_str().unwrap(), "Backfill complete");

    // Mention should now be in the DB
    let count: i64 =
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM mentions WHERE channel = 'github'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
            .0;
    assert_eq!(count, 1);

    std::env::remove_var("GITHUB_BASE_URL");
}

#[tokio::test]
#[serial]
async fn backfill_skips_duplicate_mentions() {
    let mock_server = MockServer::start();

    // Return same issue ID on every call
    mock_server.mock(|when, then| {
        when.method(GET).path("/search/issues");
        then.status(200).json_body(serde_json::json!({
            "items": [{
                "id": 99002,
                "title": "fern issue dedup test",
                "body": "fern body",
                "html_url": "https://github.com/ext/repo/issues/1",
                "user": { "login": "user1", "html_url": "https://github.com/user1" },
                "created_at": chrono::Utc::now().to_rfc3339(),
                "state": "open",
                "repository_url": "https://api.github.com/repos/ext/repo"
            }]
        }));
    });

    std::env::set_var("GITHUB_BASE_URL", mock_server.url(""));

    let app = common::spawn_app().await;

    // Create workspace + monitor
    let ws_resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "dedup-test"}))
        .send()
        .await
        .unwrap();
    let ws: serde_json::Value = ws_resp.json().await.unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    app.post("/api/monitors")
        .json(&serde_json::json!({"workspace_id": ws_id, "terms": ["fern"]}))
        .send()
        .await
        .unwrap();

    app.put("/api/channels/github")
        .json(&serde_json::json!({"enabled": true, "credentials": {"token": ""}}))
        .send()
        .await
        .unwrap();

    // Backfill twice
    for _ in 0..2 {
        app.post("/api/channels/github/backfill")
            .json(&serde_json::json!({"days": 7}))
            .send()
            .await
            .unwrap();
    }

    // Should still only have 1 mention
    let count: i64 =
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM mentions WHERE channel = 'github'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
            .0;
    assert_eq!(count, 1);

    std::env::remove_var("GITHUB_BASE_URL");
}
