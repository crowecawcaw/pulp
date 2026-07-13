use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::db::repos::traits::ChannelConfig;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ChannelBody {
    pub enabled: Option<bool>,
    pub credentials: Option<serde_json::Value>,
    pub poll_interval: Option<i64>,
}

/// `GET /api/channels` — list every channel config (global, not workspace-scoped).
#[utoipa::path(
    get,
    path = "/api/channels",
    tag = "channels",
    operation_id = "listChannels",
    responses((status = 200, body = Vec<ChannelConfig>))
)]
pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ChannelConfig>>, AppError> {
    let channels = state.channels.list().await?;
    Ok(Json(channels))
}

/// `GET /api/channels/:channel` — fetch a single channel config by name.
/// Returns 404 if the channel has never been configured.
#[utoipa::path(
    get,
    path = "/api/channels/{channel}",
    tag = "channels",
    operation_id = "getChannel",
    params(("channel" = String, Path, description = "Channel name")),
    responses((status = 200, body = ChannelConfig), (status = 404))
)]
pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
) -> Result<Json<ChannelConfig>, AppError> {
    let config = state
        .channels
        .get(&channel)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(config))
}

/// `PUT /api/channels/:channel` — create or update a channel config. Body fields
/// are all optional: `enabled`, `poll_interval` (seconds, default 900), and
/// `credentials` (channel-specific JSON). For Reddit the only (optional) credential
/// is `{ "user_agent" }` — Reddit's public search needs no client id/secret. For
/// GitHub the credentials are a `{ "token", ... }` filter object. When `enabled` is
/// omitted the channel's current enabled state is preserved. When `credentials` is
/// omitted the previously-stored credentials are preserved (not wiped) — pass an
/// explicit `credentials` object to change them.
#[utoipa::path(
    put,
    path = "/api/channels/{channel}",
    tag = "channels",
    params(("channel" = String, Path, description = "Channel name")),
    request_body = ChannelBody,
    responses((status = 200, body = ChannelConfig))
)]
pub async fn upsert(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
    Json(body): Json<ChannelBody>,
) -> Result<Json<ChannelConfig>, AppError> {
    let current_enabled = if let Some(e) = body.enabled {
        e
    } else {
        state
            .channels
            .get(&channel)
            .await
            .ok()
            .flatten()
            .map(|c| c.enabled)
            .unwrap_or(false)
    };

    let poll_interval = body.poll_interval.unwrap_or(900);

    let config = state
        .channels
        .upsert(&channel, current_enabled, body.credentials, poll_interval)
        .await?;
    Ok(Json(config))
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct CleanupBody {
    pub dry_run: bool,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct MentionSample {
    pub id: String,
    pub repo: Option<String>,
    pub author: Option<String>,
    pub title: String,
    pub url: String,
}

/// Preview returned by a dry-run cleanup: how many mentions would be removed,
/// plus up to 10 samples.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct CleanupPreview {
    pub count: usize,
    pub sample: Vec<MentionSample>,
}

/// Result of a real (non-dry-run) cleanup: how many mentions were deleted.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct CleanupResult {
    pub deleted: u64,
}

/// A cleanup call returns a preview on `dry_run` and a delete count otherwise.
/// Untagged so the wire shape is exactly one of the two inner objects.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum CleanupResponse {
    Preview(CleanupPreview),
    Result(CleanupResult),
}

#[utoipa::path(
    post,
    path = "/api/channels/{channel}/cleanup",
    tag = "channels",
    params(("channel" = String, Path, description = "Channel name")),
    request_body = CleanupBody,
    responses((status = 200, body = CleanupResponse))
)]
pub async fn cleanup(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
    Json(body): Json<CleanupBody>,
) -> Result<Json<CleanupResponse>, AppError> {
    // 1. Get channel config → parse GitHubSettings (for "github"), default for others
    let config = state
        .channels
        .get(&channel)
        .await?
        .ok_or(AppError::NotFound)?;
    let settings: crate::collectors::github_filter::GitHubSettings =
        serde_json::from_value(config.credentials.clone()).unwrap_or_default();

    // 2. Load all mentions for this channel
    let mentions = state.mentions.list_for_channel(&channel).await?;

    // 3. Apply is_ignored filter
    let to_remove: Vec<_> = mentions
        .iter()
        .filter(|m| {
            let repo = m
                .platform_meta
                .get("repo")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let author = m.author_name.as_deref().unwrap_or("");
            crate::collectors::github_filter::is_ignored(&settings, repo, author)
        })
        .collect();

    if body.dry_run {
        // Return preview
        let sample: Vec<MentionSample> = to_remove
            .iter()
            .take(10)
            .map(|m| {
                let repo = m
                    .platform_meta
                    .get("repo")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                MentionSample {
                    id: m.id.clone(),
                    repo,
                    author: m.author_name.clone(),
                    title: m.content_text.chars().take(120).collect(),
                    url: m.content_url.clone(),
                }
            })
            .collect();
        Ok(Json(CleanupResponse::Preview(CleanupPreview {
            count: to_remove.len(),
            sample,
        })))
    } else {
        // Delete
        let ids: Vec<String> = to_remove.iter().map(|m| m.id.clone()).collect();
        let deleted = state.mentions.delete_many(&ids).await?;
        Ok(Json(CleanupResponse::Result(CleanupResult { deleted })))
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct BackfillBody {
    pub days: u32,
}

/// Result of a channel backfill: a human-readable status message.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct BackfillResult {
    pub message: String,
}

#[utoipa::path(
    post,
    path = "/api/channels/{channel}/backfill",
    tag = "channels",
    params(("channel" = String, Path, description = "Channel name")),
    request_body = BackfillBody,
    responses((status = 200, body = BackfillResult))
)]
pub async fn backfill(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
    Json(body): Json<BackfillBody>,
) -> Result<Json<BackfillResult>, AppError> {
    let since = chrono::Utc::now().timestamp() - body.days as i64 * 86_400;
    crate::collectors::run_once_since(&state, &channel, since).await;
    Ok(Json(BackfillResult {
        message: "Backfill complete".to_string(),
    }))
}
