//! The API is unauthenticated by design (see docs/THREAT_MODEL.md): the
//! browser's same-origin policy is what stops a malicious web page from
//! reading data or rewriting config on a listener it can reach. That defense
//! only holds if the server never emits CORS allow headers — a permissive
//! `Access-Control-Allow-Origin` would hand every website read/write access
//! to the API. These tests fail if anyone reintroduces a CORS layer.

mod common;

/// A cross-origin GET must not carry `Access-Control-Allow-Origin`, so the
/// browser refuses to hand the response body (mentions, channel credentials)
/// to the page.
#[tokio::test]
async fn cross_origin_reads_are_not_granted() {
    let app = common::spawn_app().await;

    for path in ["/api/workspaces", "/api/channels", "/api/config/ai"] {
        let resp = app
            .get(path)
            .header("Origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "{path} should be reachable (perimeter security, not auth)"
        );
        assert!(
            resp.headers().get("access-control-allow-origin").is_none(),
            "{path} must not grant cross-origin reads"
        );
    }
}

/// A CORS preflight (browser asking permission for a cross-origin PUT with a
/// JSON body) must come back without allow headers, so the browser never sends
/// the actual state-changing request.
#[tokio::test]
async fn cross_origin_preflight_is_not_granted() {
    let app = common::spawn_app().await;

    let resp = app
        .client
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/api/config/ai", app.base_url),
        )
        .header("Origin", "https://evil.example")
        .header("Access-Control-Request-Method", "PUT")
        .header("Access-Control-Request-Headers", "content-type")
        .send()
        .await
        .unwrap();

    for header in [
        "access-control-allow-origin",
        "access-control-allow-methods",
        "access-control-allow-headers",
    ] {
        assert!(
            resp.headers().get(header).is_none(),
            "preflight must not grant `{header}`"
        );
    }
}

/// State-changing endpoints must require a JSON body. A cross-origin HTML form
/// can only send "simple" content types without a preflight; rejecting those
/// with 4xx means such a form post can't alter config even though the request
/// reaches the server.
#[tokio::test]
async fn form_encoded_writes_are_rejected() {
    let app = common::spawn_app().await;

    let resp = app
        .client
        .put(format!("{}/api/config/ai", app.base_url))
        .header("Origin", "https://evil.example")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("base_url=https%3A%2F%2Fevil.example%2Fv1")
        .send()
        .await
        .unwrap();

    assert!(
        resp.status().is_client_error(),
        "form-encoded write must be rejected, got {}",
        resp.status()
    );
}
