//! Notifier fan-out tests: every feed-visible, un-notified mention is delivered
//! to ALL notifications in its workspace, exactly once, scoped to that
//! workspace. Webhook delivery hits a mock sink; a webpush 410 prunes the
//! notification. Mocks only the external delivery boundary (`mock_sink` /
//! `mock_push`); the rest is the production notifier path.

mod common;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::SecretKey;
use pulp::db::repos::traits::NewMention;
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

/// Create a monitor in a workspace and return its id.
async fn create_monitor(app: &common::TestApp, ws_id: &str) -> String {
    let resp = app
        .post("/api/monitors")
        .json(&json!({ "workspace_id": ws_id, "terms": ["x"], "channels": ["reddit"] }))
        .send()
        .await
        .unwrap();
    let m: Value = resp.json().await.unwrap();
    m["id"].as_str().unwrap().to_string()
}

/// Insert a mention for a monitor, optionally with an AI verdict.
async fn insert_mention(
    app: &common::TestApp,
    monitor_id: &str,
    external_id: &str,
    ai_verdict: Option<&str>,
) -> String {
    let m = app
        .state
        .mentions
        .insert(NewMention {
            monitor_id: monitor_id.to_string(),
            channel: "reddit".to_string(),
            external_id: external_id.to_string(),
            content_text: "a mention worth notifying about".to_string(),
            content_url: format!("https://reddit.com/{}", external_id),
            author_name: Some("tester".to_string()),
            author_url: None,
            published_at: Some(chrono::Utc::now().timestamp() - 60),
            platform_meta: json!({}),
            ai_verdict: ai_verdict.map(String::from),
        })
        .await
        .unwrap();
    m.id
}

/// Register a webhook notification pointed at `url`.
async fn add_webhook(app: &common::TestApp, ws_id: &str, url: &str) {
    app.post("/api/notifications")
        .json(&json!({ "workspace_id": ws_id, "kind": "webhook", "config": { "url": url } }))
        .send()
        .await
        .unwrap();
}

/// A browser-shaped webpush subscription config with a real P-256 public key
/// (so encryption succeeds) pointed at `endpoint`.
fn webpush_config(endpoint: &str) -> Value {
    let secret = SecretKey::random(&mut rand_core::OsRng);
    let p256dh = secret.public_key().to_encoded_point(false);
    json!({
        "endpoint": endpoint,
        "p256dh": URL_SAFE_NO_PAD.encode(p256dh.as_bytes()),
        "auth": URL_SAFE_NO_PAD.encode([0x42u8; 16]),
    })
}

#[tokio::test]
async fn feed_mention_fans_out_then_marks_notified() {
    let app = common::spawn_app().await;
    let sink = common::mock_sink::spawn().await;
    let ws = create_workspace(&app, "WS").await;
    let monitor = create_monitor(&app, &ws).await;
    add_webhook(&app, &ws, &sink.url).await;

    let mid = insert_mention(&app, &monitor, "t3_1", None).await;

    // First pass delivers to the workspace's webhook.
    pulp::notifier::run_notify_pass(&app.state).await;
    assert_eq!(sink.count(), 1, "one delivery on the first pass");
    let payload = &sink.payloads()[0];
    assert_eq!(payload["id"], mid);
    assert_eq!(payload["channel"], "reddit");

    // Second pass does NOT re-deliver (notified_at is set).
    pulp::notifier::run_notify_pass(&app.state).await;
    assert_eq!(sink.count(), 1, "no re-delivery on the second pass");
}

#[tokio::test]
async fn mention_only_hits_its_own_workspaces_notifications() {
    let app = common::spawn_app().await;
    let sink_a = common::mock_sink::spawn().await;
    let sink_b = common::mock_sink::spawn().await;

    let ws_a = create_workspace(&app, "A").await;
    let ws_b = create_workspace(&app, "B").await;
    let mon_a = create_monitor(&app, &ws_a).await;
    add_webhook(&app, &ws_a, &sink_a.url).await;
    add_webhook(&app, &ws_b, &sink_b.url).await;

    // A mention in workspace A only.
    insert_mention(&app, &mon_a, "t3_a", None).await;

    pulp::notifier::run_notify_pass(&app.state).await;
    assert_eq!(sink_a.count(), 1, "A's notification fired");
    assert_eq!(sink_b.count(), 0, "B's notification must NOT fire");
}

/// Dedup is per-monitor (`UNIQUE(monitor_id, channel, external_id)`), so the
/// same external post matching monitors in two different workspaces is
/// stored as two independent mention rows. The notifier must fan each row out
/// to its OWN workspace's webhook — both must fire, each exactly once.
#[tokio::test]
async fn same_external_post_across_workspaces_fans_out_to_both() {
    let app = common::spawn_app().await;
    let sink_a = common::mock_sink::spawn().await;
    let sink_b = common::mock_sink::spawn().await;

    let ws_a = create_workspace(&app, "A").await;
    let ws_b = create_workspace(&app, "B").await;
    let mon_a = create_monitor(&app, &ws_a).await;
    let mon_b = create_monitor(&app, &ws_b).await;
    add_webhook(&app, &ws_a, &sink_a.url).await;
    add_webhook(&app, &ws_b, &sink_b.url).await;

    // Same channel + external_id, different monitor (and workspace) — allowed
    // now that dedup is scoped per monitor, not global.
    let mid_a = insert_mention(&app, &mon_a, "t3_shared", None).await;
    let mid_b = insert_mention(&app, &mon_b, "t3_shared", None).await;
    assert_ne!(mid_a, mid_b, "each workspace must get its own mention row");

    pulp::notifier::run_notify_pass(&app.state).await;

    assert_eq!(sink_a.count(), 1, "workspace A's webhook must fire");
    assert_eq!(sink_b.count(), 1, "workspace B's webhook must fire");
    assert_eq!(sink_a.payloads()[0]["id"], mid_a);
    assert_eq!(sink_b.payloads()[0]["id"], mid_b);
    assert_eq!(
        sink_a.payloads()[0]["content_url"],
        sink_b.payloads()[0]["content_url"],
        "both deliveries are for the same underlying external post"
    );

    // Fire-once still holds per row: a second pass replays neither.
    pulp::notifier::run_notify_pass(&app.state).await;
    assert_eq!(sink_a.count(), 1);
    assert_eq!(sink_b.count(), 1);
}

#[tokio::test]
async fn rejected_and_pending_mentions_do_not_fan_out() {
    let app = common::spawn_app().await;
    let sink = common::mock_sink::spawn().await;
    let ws = create_workspace(&app, "WS").await;
    let monitor = create_monitor(&app, &ws).await;
    add_webhook(&app, &ws, &sink.url).await;

    // AI-rejected and still-pending mentions are not feed-visible.
    insert_mention(&app, &monitor, "t3_rej", Some("rejected")).await;
    insert_mention(&app, &monitor, "t3_pend", Some("pending")).await;
    // An accepted one IS feed-visible.
    insert_mention(&app, &monitor, "t3_ok", Some("accepted")).await;

    pulp::notifier::run_notify_pass(&app.state).await;
    assert_eq!(sink.count(), 1, "only the accepted mention fans out");
    assert_eq!(
        sink.payloads()[0]["content_url"],
        "https://reddit.com/t3_ok"
    );
}

#[tokio::test]
async fn adding_a_notification_does_not_replay_history() {
    let app = common::spawn_app().await;
    let sink = common::mock_sink::spawn().await;
    let ws = create_workspace(&app, "WS").await;
    let monitor = create_monitor(&app, &ws).await;

    // A mention exists and is fanned out (to nobody) before any notification.
    insert_mention(&app, &monitor, "t3_old", None).await;
    pulp::notifier::run_notify_pass(&app.state).await;
    // Marked notified despite no notifications existing yet.

    // Now add a notification — the pre-existing mention must NOT replay.
    add_webhook(&app, &ws, &sink.url).await;
    pulp::notifier::run_notify_pass(&app.state).await;
    assert_eq!(
        sink.count(),
        0,
        "history is not replayed for a new notification"
    );

    // A brand-new mention does fire to it.
    insert_mention(&app, &monitor, "t3_new", None).await;
    pulp::notifier::run_notify_pass(&app.state).await;
    assert_eq!(sink.count(), 1, "new mentions still fire");
}

#[tokio::test]
async fn webpush_410_prunes_the_notification() {
    let app = common::spawn_app().await;
    let push = common::mock_push::spawn_with_status(410).await;
    let ws = create_workspace(&app, "WS").await;
    let monitor = create_monitor(&app, &ws).await;

    app.post("/api/notifications")
        .json(
            &json!({ "workspace_id": ws, "kind": "webpush", "config": webpush_config(&push.url) }),
        )
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

    insert_mention(&app, &monitor, "t3_1", None).await;
    pulp::notifier::run_notify_pass(&app.state).await;

    // The dead subscription was attempted once, then pruned.
    assert_eq!(push.count(), 1);
    assert!(
        app.state
            .notifications
            .list_by_workspace(&ws)
            .await
            .unwrap()
            .is_empty(),
        "the 410 webpush notification was pruned"
    );
}

#[tokio::test]
async fn test_endpoint_delivers_to_workspace_webhook() {
    let app = common::spawn_app().await;
    let sink = common::mock_sink::spawn().await;
    let ws = create_workspace(&app, "WS").await;
    add_webhook(&app, &ws, &sink.url).await;

    let resp = app
        .post(&format!("/api/notifications/test?workspace_id={}", ws))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let result: Value = resp.json().await.unwrap();
    assert_eq!(result["delivered"].as_u64().unwrap(), 1);
    assert_eq!(sink.count(), 1);
    assert_eq!(sink.payloads()[0]["test"], true);
}

/// The browser still needs the VAPID public key before it can subscribe.
#[tokio::test]
async fn vapid_public_key_is_served() {
    let app = common::spawn_app().await;
    let resp = app.get("/api/push/vapid-public-key").send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let key = body["key"].as_str().unwrap();
    let bytes = URL_SAFE_NO_PAD.decode(key).unwrap();
    assert_eq!(bytes.len(), 65);
    assert_eq!(bytes[0], 0x04);
}
