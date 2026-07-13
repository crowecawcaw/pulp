use crate::{collectors, error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

#[utoipa::path(
    post,
    path = "/api/admin/collect/{channel}",
    tag = "admin",
    params(("channel" = String, Path, description = "Channel name")),
    responses((status = 200, description = "Collection run triggered"))
)]
pub async fn trigger_collect(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
) -> Result<StatusCode, AppError> {
    collectors::run_once(&state, &channel).await;
    Ok(StatusCode::OK)
}

/// Run a single notifier pass synchronously: fan every feed-visible,
/// un-notified mention out to its workspace's notifications, then mark them
/// notified. Mirrors the background loop so the pipeline can be driven
/// deterministically.
#[utoipa::path(
    post,
    path = "/api/admin/notify",
    tag = "admin",
    responses((status = 200, description = "Notify pass executed"))
)]
pub async fn trigger_notify(State(state): State<Arc<AppState>>) -> Result<StatusCode, AppError> {
    crate::notifier::run_notify_pass(&state).await;
    Ok(StatusCode::OK)
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct BackfillRequest {
    pub channel: Option<String>,
    pub since: i64,
}

#[utoipa::path(
    post,
    path = "/api/admin/backfill",
    tag = "admin",
    request_body = BackfillRequest,
    responses(
        (status = 200, description = "Single-channel backfill complete"),
        (status = 202, description = "All-channel backfill accepted")
    )
)]
pub async fn trigger_backfill(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BackfillRequest>,
) -> Result<StatusCode, AppError> {
    match body.channel {
        Some(channel) => {
            // Single channel: run synchronously
            collectors::run_once_since(&state, &channel, body.since).await;
            Ok(StatusCode::OK)
        }
        None => {
            // All channels: spawn one task per channel, return 202 immediately
            for channel in collectors::CHANNELS {
                let state_clone = state.clone();
                let since = body.since;
                let ch = channel.to_string();
                tokio::spawn(async move {
                    collectors::run_once_since(&state_clone, &ch, since).await;
                });
            }
            Ok(StatusCode::ACCEPTED)
        }
    }
}
