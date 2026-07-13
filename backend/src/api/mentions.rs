use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::db::repos::traits::{Mention, MentionFilter, PendingCount};
use crate::error::AppError;
use crate::state::AppState;

#[derive(Deserialize, IntoParams)]
pub struct MentionQuery {
    pub workspace_id: Option<String>,
    pub channel: Option<String>,
    pub monitor_id: Option<String>,
    pub limit: Option<i64>,
    /// Upper bound (exclusive) on the effective timestamp (`published_at`,
    /// or `ingested_at` when `published_at` is null). For the feed's
    /// keyset-pagination cursor, pair this with `before_id` — the `id` of
    /// the last item on the previous page — so rows tied on the effective
    /// timestamp are still visited exactly once when paging one page at a
    /// time; `before` alone has no tiebreak.
    pub before: Option<i64>,
    /// Tiebreaker id paired with `before` for keyset pagination — the `id`
    /// of the last item on the previous page. See `before`.
    pub before_id: Option<String>,
    /// Lower bound: return mentions published at or after this epoch.
    pub since: Option<i64>,
    /// Filter by read state: `true` = read only, `false` = unread only,
    /// omitted = both.
    pub read: Option<bool>,
    /// AI verdict filter: omitted/`visible` = feed default (no verdict or
    /// `accepted`); `all` = everything; `pending` / `accepted` / `rejected`
    /// = exactly that verdict.
    pub ai: Option<String>,
}

/// A page of mentions plus a cursor hint for keyset pagination.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct MentionPage {
    pub items: Vec<Mention>,
    pub has_more: bool,
}

/// `GET /api/mentions` — list mentions (keyset-paginated by the compound
/// `before`/`before_id` cursor).
#[utoipa::path(
    get,
    path = "/api/mentions",
    tag = "mentions",
    operation_id = "listMentions",
    params(MentionQuery),
    responses((status = 200, body = MentionPage))
)]
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<MentionQuery>,
) -> Result<Json<MentionPage>, AppError> {
    let (ai_verdict, ai_visible_only) = match q.ai.as_deref() {
        None | Some("visible") => (None, true),
        Some("all") => (None, false),
        Some(v) => (Some(v.to_string()), false),
    };

    let filter = MentionFilter {
        workspace_id: q.workspace_id,
        channel: q.channel,
        limit: q.limit,
        before: q.before,
        before_id: q.before_id,
        since: q.since,
        monitor_id: q.monitor_id,
        read: q.read,
        ai_verdict,
        ai_visible_only,
    };

    let (items, has_more) = state.mentions.list(filter).await?;
    Ok(Json(MentionPage { items, has_more }))
}

#[derive(Deserialize, IntoParams)]
pub struct PendingCountQuery {
    /// Scope the count to a single workspace; omitted = all workspaces.
    pub workspace_id: Option<String>,
}

/// `GET /api/mentions/pending-count` — size + age of the AI-filter backlog,
/// driving the feed's "N pending AI filter" banner.
#[utoipa::path(
    get,
    path = "/api/mentions/pending-count",
    tag = "mentions",
    operation_id = "mentionsPendingCount",
    params(PendingCountQuery),
    responses((status = 200, body = PendingCount))
)]
pub async fn pending_count(
    State(state): State<Arc<AppState>>,
    Query(q): Query<PendingCountQuery>,
) -> Result<Json<PendingCount>, AppError> {
    let pc = state
        .mentions
        .count_ai_pending(q.workspace_id.as_deref())
        .await?;
    Ok(Json(pc))
}

/// `GET /api/mentions/:id` — fetch a single mention (used by the mention
/// detail page that web-push notifications deep-link to).
#[utoipa::path(
    get,
    path = "/api/mentions/{id}",
    tag = "mentions",
    operation_id = "getMention",
    params(("id" = String, Path, description = "Mention id")),
    responses(
        (status = 200, body = Mention),
        (status = 404, description = "Mention not found")
    )
)]
pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Mention>, AppError> {
    let mention = state.mentions.get(&id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(mention))
}

/// Body for [`set_read`].
#[derive(Serialize, Deserialize, ToSchema)]
pub struct SetReadRequest {
    /// `true` marks the mention read, `false` marks it unread.
    pub read: bool,
}

/// `PUT /api/mentions/:id/read` — mark a mention read or unread.
#[utoipa::path(
    put,
    path = "/api/mentions/{id}/read",
    tag = "mentions",
    operation_id = "setMentionRead",
    params(("id" = String, Path, description = "Mention id")),
    request_body = SetReadRequest,
    responses(
        (status = 200, body = Mention),
        (status = 404, description = "Mention not found")
    )
)]
pub async fn set_read(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SetReadRequest>,
) -> Result<Json<Mention>, AppError> {
    let mention = state.mentions.set_read(&id, body.read).await?;
    Ok(Json(mention))
}
