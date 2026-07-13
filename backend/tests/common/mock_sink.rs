//! A mock webhook sink — the external notification boundary. It records every
//! JSON payload it receives so tests can assert exactly which mentions were
//! delivered (and how many times).

use axum::{extract::State, routing::post, Json, Router};
use std::future::IntoFuture;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Clone)]
pub struct MockSink {
    /// Payloads received, in arrival order.
    pub received: Arc<Mutex<Vec<serde_json::Value>>>,
    /// URL to use as a webhook destination's `config.url`.
    pub url: String,
}

impl MockSink {
    pub fn count(&self) -> usize {
        self.received.lock().unwrap().len()
    }

    pub fn payloads(&self) -> Vec<serde_json::Value> {
        self.received.lock().unwrap().clone()
    }
}

pub async fn spawn() -> MockSink {
    let received: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));

    let app = Router::new()
        .route("/hook", post(handle))
        .with_state(received.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app).into_future());

    MockSink {
        received,
        url: format!("http://{}/hook", addr),
    }
}

async fn handle(
    State(received): State<Arc<Mutex<Vec<serde_json::Value>>>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    received.lock().unwrap().push(body);
    Json(serde_json::json!({ "ok": true }))
}
