use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::collectors::scheduler::{
    summarize_targets, target_health, ChannelHealthSummary, TargetHealth,
};
use crate::db::repos::traits::{BackfillJob, CollectorTarget};
use crate::error::AppError;
use crate::state::AppState;

/// Per-channel collection status: every durable target with its watermark and
/// sticky failure status, plus a summary of its open backfill jobs.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct TargetStatus {
    #[serde(flatten)]
    pub target: CollectorTarget,
    /// Coarse health derived from the sticky status fields (the same classifier
    /// the channel-level degraded banner counts), computed live at request time.
    pub health: TargetHealth,
    /// Open backfill jobs for this target (durable, survive restart).
    pub open_jobs: Vec<BackfillJob>,
}

/// A snapshot of a channel's live adaptive rate-limiter (the shared lane all of
/// the channel's targets pace through). Surfaced so the UI can watch the
/// interval converge as the limiter adapts.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct ThrottleState {
    /// The current effective issuance rate, in requests per minute (`rate_per_sec * 60`).
    pub rate_per_min: f64,
    /// The current minimum spacing between requests, in seconds.
    pub interval_secs: f64,
    /// Whether issuance is currently gated to a future instant (a hard
    /// `Retry-After` pause, or simply the next paced slot).
    pub paused: bool,
}

/// The response for `GET /api/channels/{channel}/targets`.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct TargetsResponse {
    pub channel: String,
    pub targets: Vec<TargetStatus>,
    /// Per-health counts + a degraded banner message, computed from the same
    /// classifier as each target's `health`, so the banner can't disagree with
    /// the per-target list.
    pub summary: ChannelHealthSummary,
    /// Live state of the channel's shared rate-limiter lane, if one has been
    /// materialized yet. `None` before the first collection pass and for
    /// simple-poller channels that don't pace through a lane.
    pub throttle: Option<ThrottleState>,
}

/// `GET /api/channels/{channel}/targets` — per-target collection status for a
/// channel (watermark, last success/attempt, consecutive failures, last error,
/// derived health) plus each target's open backfill jobs. Backend-only status view
/// for the targeted (auto-recovering) collector pipeline.
#[utoipa::path(
    get,
    path = "/api/channels/{channel}/targets",
    tag = "channels",
    operation_id = "listChannelTargets",
    params(("channel" = String, Path, description = "Channel name")),
    responses((status = 200, body = TargetsResponse))
)]
pub async fn list_targets(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
) -> Result<Json<TargetsResponse>, AppError> {
    let targets = state.collector_targets.list_targets(&channel).await?;
    // One classifier for both the per-target health and the summary, so the
    // banner can't disagree with the table.
    let summary = summarize_targets(&targets);
    let mut out = Vec::with_capacity(targets.len());
    for target in targets {
        let open_jobs = state
            .collector_targets
            .list_open_jobs_for_target(&target.id)
            .await?;
        let health = target_health(&target);
        out.push(TargetStatus {
            target,
            health,
            open_jobs,
        });
    }
    // The lane key == the channel name (see `collectors::scheduler` /
    // `reddit::plan_targets`). Peek (not lane) so a GET doesn't materialize a
    // lane as a side effect.
    let throttle = state.throttles.peek(&channel).map(|t| {
        let s = t.limiter().state();
        ThrottleState {
            rate_per_min: s.rate_per_sec * 60.0,
            interval_secs: s.interval_secs,
            paused: s.paused,
        }
    });
    Ok(Json(TargetsResponse {
        channel,
        targets: out,
        summary,
        throttle,
    }))
}
