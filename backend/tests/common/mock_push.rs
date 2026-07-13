//! A mock Web Push service — the external boundary a browser's `endpoint`
//! points at. Unlike `mock_sink` (which expects JSON), a push delivery is an
//! `aes128gcm` octet-stream with VAPID auth headers, so this captures the raw
//! body and the relevant headers and lets the test choose the HTTP status to
//! return (201 for a live subscription, 410 to exercise dead-sub pruning).

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Clone)]
pub struct ReceivedPush {
    pub authorization: Option<String>,
    pub content_encoding: Option<String>,
    pub ttl: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Clone)]
pub struct MockPush {
    /// Requests received, in arrival order.
    pub received: Arc<Mutex<Vec<ReceivedPush>>>,
    /// URL to register as a subscription `endpoint`.
    pub url: String,
}

impl MockPush {
    pub fn count(&self) -> usize {
        self.received.lock().unwrap().len()
    }

    pub fn requests(&self) -> Vec<ReceivedPush> {
        self.received.lock().unwrap().clone()
    }
}

/// Spawn a mock push service that responds to deliveries with `status`.
pub async fn spawn_with_status(status: u16) -> MockPush {
    let received: Arc<Mutex<Vec<ReceivedPush>>> = Arc::new(Mutex::new(Vec::new()));

    let app = Router::new()
        .route("/push", post(handle))
        .with_state((received.clone(), status));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app).into_future());

    MockPush {
        received,
        url: format!("http://{}/push", addr),
    }
}

/// A mock that accepts deliveries (HTTP 201, as real push services do).
pub async fn spawn() -> MockPush {
    spawn_with_status(201).await
}

async fn handle(
    State((received, status)): State<(Arc<Mutex<Vec<ReceivedPush>>>, u16)>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };
    received.lock().unwrap().push(ReceivedPush {
        authorization: header("authorization"),
        content_encoding: header("content-encoding"),
        ttl: header("ttl"),
        body: body.to_vec(),
    });
    StatusCode::from_u16(status).unwrap()
}
