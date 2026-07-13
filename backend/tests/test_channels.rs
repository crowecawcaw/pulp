mod common;

#[tokio::test]
async fn test_list_channels() {
    let app = common::spawn_app().await;
    let resp = app.get("/api/channels").send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let channels: serde_json::Value = resp.json().await.unwrap();
    // Migration seeds exactly the channels with a real collector — see
    // `collectors::CHANNELS`. No dead rows for channels nothing collects.
    let names: Vec<&str> = channels
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["channel"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 3);
    for expected in pulp::collectors::CHANNELS {
        assert!(
            names.contains(expected),
            "expected seeded channel {expected:?} in {names:?}"
        );
    }
}

#[tokio::test]
async fn test_enable_channel() {
    let app = common::spawn_app().await;

    let resp = app
        .put("/api/channels/hackernews")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let channel: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(channel["enabled"].as_bool().unwrap(), true);
    assert_eq!(channel["channel"].as_str().unwrap(), "hackernews");
}

#[tokio::test]
async fn test_set_credentials() {
    let app = common::spawn_app().await;

    let resp = app
        .put("/api/channels/reddit")
        .json(&serde_json::json!({
            "enabled": false,
            "credentials": {
                "client_id": "test_client_id",
                "client_secret": "test_client_secret"
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let channel: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(channel["channel"].as_str().unwrap(), "reddit");
    let creds = &channel["credentials"];
    assert_eq!(creds["client_id"].as_str().unwrap(), "test_client_id");
    assert_eq!(
        creds["client_secret"].as_str().unwrap(),
        "test_client_secret"
    );
}

// Regression: `ChannelRepo::upsert` used to overwrite `credentials`
// unconditionally, so a `PUT` that only toggled `enabled` (omitting
// `credentials` entirely) wiped any previously-stored token. The body field
// is now `Option<serde_json::Value>`, and omitting it must preserve whatever
// is already stored — only an explicit `credentials` object should change it.
#[tokio::test]
async fn test_put_channel_without_credentials_preserves_stored_credentials() {
    let app = common::spawn_app().await;

    // Store real credentials first.
    let resp = app
        .put("/api/channels/github")
        .json(&serde_json::json!({
            "enabled": true,
            "credentials": { "token": "nimbus-gh-pat" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let channel: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        channel["credentials"]["token"].as_str().unwrap(),
        "nimbus-gh-pat"
    );

    // Toggle `enabled` only — `credentials` is omitted from the body, not
    // sent as `null`, mirroring how the Channels page's enable/disable
    // toggle calls this endpoint.
    let resp = app
        .put("/api/channels/github")
        .json(&serde_json::json!({ "enabled": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let channel: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(channel["enabled"].as_bool().unwrap(), false);
    assert_eq!(
        channel["credentials"]["token"].as_str().unwrap(),
        "nimbus-gh-pat",
        "omitting `credentials` on PUT must not wipe the stored token"
    );

    // A GET confirms the same state was actually persisted, not just echoed
    // back in the PUT response.
    let resp = app.get("/api/channels/github").send().await.unwrap();
    let channel: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        channel["credentials"]["token"].as_str().unwrap(),
        "nimbus-gh-pat"
    );
}
