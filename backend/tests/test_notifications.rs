//! Notification repo tests: create/list/delete per workspace, endpoint pruning,
//! and cascade on workspace delete. Driven through the HTTP API + the repo on
//! the shared in-memory database.

mod common;

use serde_json::{json, Value};

async fn create_workspace(app: &common::TestApp, name: &str) -> String {
    let resp = app
        .post("/api/workspaces")
        .json(&json!({ "name": name }))
        .send()
        .await
        .unwrap();
    let ws: Value = resp.json().await.unwrap();
    ws["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn create_list_delete_per_workspace() {
    let app = common::spawn_app().await;
    let ws_a = create_workspace(&app, "A").await;
    let ws_b = create_workspace(&app, "B").await;

    // Create a webhook in A.
    let resp = app
        .post("/api/notifications")
        .json(&json!({
            "workspace_id": ws_a,
            "kind": "webhook",
            "config": { "url": "https://example.com/a" },
            "label": "A hook",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let n: Value = resp.json().await.unwrap();
    assert_eq!(n["kind"], "webhook");
    assert_eq!(n["config"]["url"], "https://example.com/a");
    let id = n["id"].as_str().unwrap().to_string();

    // List for A returns it; list for B is empty (workspace-scoped).
    let a_items = app
        .get(&format!("/api/notifications?workspace_id={}", ws_a))
        .send()
        .await
        .unwrap()
        .json::<Vec<Value>>()
        .await
        .unwrap();
    assert_eq!(a_items.len(), 1);
    let b_items = app
        .get(&format!("/api/notifications?workspace_id={}", ws_b))
        .send()
        .await
        .unwrap()
        .json::<Vec<Value>>()
        .await
        .unwrap();
    assert!(b_items.is_empty());

    // Delete it.
    let resp = app
        .delete(&format!("/api/notifications/{}", id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    let a_items = app
        .get(&format!("/api/notifications?workspace_id={}", ws_a))
        .send()
        .await
        .unwrap()
        .json::<Vec<Value>>()
        .await
        .unwrap();
    assert!(a_items.is_empty());
}

#[tokio::test]
async fn rejects_unknown_kind() {
    let app = common::spawn_app().await;
    let ws = create_workspace(&app, "WS").await;
    let resp = app
        .post("/api/notifications")
        .json(&json!({
            "workspace_id": ws,
            "kind": "smoke-signal",
            "config": {},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn delete_by_endpoint_prunes_only_matching_webpush() {
    let app = common::spawn_app().await;
    let ws = create_workspace(&app, "WS").await;

    // Two webpush notifications with different endpoints + one webhook.
    let mk_webpush = |endpoint: &str| {
        json!({
            "workspace_id": ws,
            "kind": "webpush",
            "config": { "endpoint": endpoint, "p256dh": "x", "auth": "y" },
        })
    };
    app.post("/api/notifications")
        .json(&mk_webpush("https://push.example/aaa"))
        .send()
        .await
        .unwrap();
    app.post("/api/notifications")
        .json(&mk_webpush("https://push.example/bbb"))
        .send()
        .await
        .unwrap();
    app.post("/api/notifications")
        .json(&json!({
            "workspace_id": ws,
            "kind": "webhook",
            "config": { "url": "https://push.example/aaa" },
        }))
        .send()
        .await
        .unwrap();

    let pruned = app
        .state
        .notifications
        .delete_by_endpoint("https://push.example/aaa")
        .await
        .unwrap();
    assert_eq!(pruned, 1, "only the matching webpush is pruned");

    let remaining = app
        .state
        .notifications
        .list_by_workspace(&ws)
        .await
        .unwrap();
    // The bbb webpush and the webhook (same URL, different kind) survive.
    assert_eq!(remaining.len(), 2);
}

#[tokio::test]
async fn deleting_workspace_cascades_notifications() {
    let app = common::spawn_app().await;
    let ws = create_workspace(&app, "WS").await;

    app.post("/api/notifications")
        .json(&json!({
            "workspace_id": ws,
            "kind": "webhook",
            "config": { "url": "https://example.com/hook" },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        app.state
            .notifications
            .list_by_workspace(&ws)
            .await
            .unwrap()
            .len(),
        1
    );

    // Delete the workspace; the ON DELETE CASCADE must remove its notifications.
    let resp = app
        .delete(&format!("/api/workspaces/{}", ws))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    assert!(app
        .state
        .notifications
        .list_by_workspace(&ws)
        .await
        .unwrap()
        .is_empty());
}
