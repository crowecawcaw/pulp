mod common;

use serial_test::serial;
use std::time::Duration;

/// Regression test for the SSE workspace filter (`GET
/// /api/mentions/stream?workspace_id=...`). It used to be a no-op — every
/// connected client saw every broadcast mention regardless of the
/// `workspace_id` query param — because `sse.rs` parsed the query but never
/// used it. It now resolves each broadcast mention's `monitor_id` to a
/// workspace and only forwards mentions belonging to the requested one.
///
/// Uses the two canonical test workspaces (Nimbus Labs / Fern): a client
/// scoped to Nimbus Labs must see mentions from Nimbus Labs's monitor and
/// must NOT see mentions from Fern's monitor, even though both are ingested
/// in the same collection pass and broadcast on the same channel.
#[tokio::test]
#[serial]
async fn sse_stream_filters_by_workspace_id() {
    let (hn_base, _spy) = common::mock_hn::spawn_counted().await;
    std::env::set_var("HACKERNEWS_BASE_URL", &hn_base);

    let app = common::spawn_app().await;

    let nimbus_ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": "Nimbus Labs" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let nimbus_ws_id = nimbus_ws["id"].as_str().unwrap().to_string();

    let fern_ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": "Fern" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let fern_ws_id = fern_ws["id"].as_str().unwrap().to_string();

    let nimbus_monitor: serde_json::Value = app
        .post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": nimbus_ws_id,
            "terms": ["nimbusdb"],
            "channels": ["hackernews"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let nimbus_monitor_id = nimbus_monitor["id"].as_str().unwrap().to_string();

    app.post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": fern_ws_id,
            "terms": ["fernlint"],
            "channels": ["hackernews"]
        }))
        .send()
        .await
        .unwrap();

    app.put("/api/channels/hackernews")
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .unwrap();

    // Connect an SSE client scoped to Nimbus Labs BEFORE triggering
    // collection — the broadcast channel has no replay buffer, so a
    // subscriber must be registered before a message is sent to see it.
    let mut resp = app
        .client
        .get(format!(
            "{}/api/mentions/stream?workspace_id={}",
            app.base_url, nimbus_ws_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // Give the server a moment to actually subscribe to the broadcast
    // channel before we fire the collection pass.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // One collection pass ingests (and broadcasts) mentions for BOTH
    // monitors — Nimbus Labs's and Fern's — in the same run.
    let collect_resp = app
        .post("/api/admin/collect/hackernews")
        .send()
        .await
        .unwrap();
    assert_eq!(collect_resp.status(), 200);
    std::env::remove_var("HACKERNEWS_BASE_URL");

    // Read whatever arrives on the Nimbus-scoped stream within a bounded
    // window (the SSE connection never closes on its own).
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, resp.chunk()).await {
            Ok(Ok(Some(bytes))) => buf.push_str(&String::from_utf8_lossy(&bytes)),
            _ => break,
        }
    }

    // Parse every `data: <json>` line out of the SSE event blocks.
    let monitor_ids: Vec<String> = buf
        .split("\n\n")
        .filter_map(|block| {
            block
                .lines()
                .find_map(|line| line.strip_prefix("data:"))
                .map(str::trim)
        })
        .filter_map(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .filter_map(|v| {
            v.get("monitor_id")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .collect();

    assert!(
        !monitor_ids.is_empty(),
        "expected at least one mention event on the Nimbus-scoped stream; got raw buffer: {buf:?}"
    );
    assert!(
        monitor_ids.iter().all(|m| m == &nimbus_monitor_id),
        "a Nimbus-scoped SSE stream must only carry Nimbus Labs's mentions, got monitor_ids: {monitor_ids:?}"
    );
}
