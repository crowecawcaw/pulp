//! Integration tests for the generic per-service logs endpoint
//! (`GET /api/logs/{service}`). The endpoint reads `<home>/server.log`; the
//! test harness points `config.home` at a sandboxed temp dir (see
//! `common::spawn_app`), so we just write a fixture log there and assert the
//! endpoint filters to the requested service and honours `limit`.

mod common;

use common::spawn_app;
use serde::Deserialize;

#[derive(Deserialize)]
struct LogResponse {
    service: String,
    lines: Vec<String>,
    exists: bool,
}

/// Write `content` to the running app's `<home>/server.log`.
fn write_server_log(app: &common::TestApp, content: &str) {
    let path = app.state.config.home.join("server.log");
    std::fs::create_dir_all(&app.state.config.home).unwrap();
    std::fs::write(path, content).unwrap();
}

const FIXTURE: &str = "\
2026-06-17T06:19:35Z  INFO pulp::collectors: Starting collector for channel: reddit
2026-06-17T06:19:36Z  INFO pulp::collectors::reddit: fetched reddit feed A
2026-06-17T06:19:37Z  INFO pulp::collectors::github: fetched repo X
2026-06-17T06:19:38Z  WARN pulp::collectors::reddit: reddit rate limited
2026-06-17T06:19:39Z  INFO pulp::ai_filter: judged a mention
";

#[tokio::test]
async fn returns_only_the_requested_service_lines() {
    let app = spawn_app().await;
    write_server_log(&app, FIXTURE);

    let body: LogResponse = app
        .get("/api/logs/reddit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body.service, "reddit");
    assert!(body.exists);
    // The two reddit-target lines plus the "channel: reddit" startup line.
    assert_eq!(body.lines.len(), 3, "got: {:?}", body.lines);
    assert!(body
        .lines
        .iter()
        .all(|l| l.to_lowercase().contains("reddit")));
    assert!(body.lines.iter().any(|l| l.contains("rate limited")));
    // No github / ai_filter lines leak in.
    assert!(!body.lines.iter().any(|l| l.contains("repo X")));
    assert!(!body.lines.iter().any(|l| l.contains("judged a mention")));
}

#[tokio::test]
async fn respects_the_limit_keeping_the_most_recent() {
    let app = spawn_app().await;
    write_server_log(&app, FIXTURE);

    let body: LogResponse = app
        .get("/api/logs/reddit?limit=1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body.lines.len(), 1);
    // Most-recent reddit line wins.
    assert!(body.lines[0].contains("rate limited"));
}

#[tokio::test]
async fn future_ai_filter_service_resolves() {
    let app = spawn_app().await;
    write_server_log(&app, FIXTURE);

    for svc in ["ai_filter", "llm"] {
        let body: LogResponse = app
            .get(&format!("/api/logs/{svc}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body.lines.len(), 1, "{svc}: {:?}", body.lines);
        assert!(body.lines[0].contains("judged a mention"));
    }
}

#[tokio::test]
async fn unknown_service_is_404() {
    let app = spawn_app().await;
    let res = app.get("/api/logs/twitter").send().await.unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn missing_log_file_yields_empty_not_error() {
    let app = spawn_app().await;
    // No server.log written.
    let res = app.get("/api/logs/reddit").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let body: LogResponse = res.json().await.unwrap();
    assert!(!body.exists);
    assert!(body.lines.is_empty());
}
