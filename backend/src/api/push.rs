//! The one Web Push endpoint the browser needs before it can subscribe: the
//! server's VAPID public key. Subscriptions themselves are now stored as
//! per-workspace `webpush` notifications (see `api::notifications`).

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::error::AppError;
use crate::state::AppState;

/// The server's VAPID public key (base64url, uncompressed P-256 point) — the
/// browser needs it as `applicationServerKey` when subscribing.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct VapidPublicKey {
    pub key: String,
}

#[utoipa::path(
    get,
    path = "/api/push/vapid-public-key",
    tag = "push",
    operation_id = "getVapidPublicKey",
    responses((status = 200, body = VapidPublicKey))
)]
pub async fn vapid_public_key(
    State(state): State<Arc<AppState>>,
) -> Result<Json<VapidPublicKey>, AppError> {
    Ok(Json(VapidPublicKey {
        key: state.vapid.public_b64.clone(),
    }))
}
