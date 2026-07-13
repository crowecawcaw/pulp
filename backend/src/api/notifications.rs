//! Per-workspace notification endpoints. A notification is a pure delivery
//! endpoint; every feed-visible mention fans out to all notifications in its
//! workspace (see `crate::notifier`). Kinds: `webpush` (added by the browser
//! after subscribing) and `webhook` (a plain URL POST).

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::db::repos::traits::{CreateNotification, Notification};
use crate::error::AppError;
use crate::state::AppState;

#[derive(Deserialize, IntoParams)]
pub struct WorkspaceQuery {
    pub workspace_id: String,
}

#[utoipa::path(
    get,
    path = "/api/notifications",
    tag = "notifications",
    operation_id = "listNotifications",
    params(WorkspaceQuery),
    responses((status = 200, body = Vec<Notification>))
)]
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<WorkspaceQuery>,
) -> Result<Json<Vec<Notification>>, AppError> {
    Ok(Json(
        state
            .notifications
            .list_by_workspace(&q.workspace_id)
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/notifications",
    tag = "notifications",
    operation_id = "createNotification",
    request_body = CreateNotification,
    responses((status = 200, body = Notification))
)]
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateNotification>,
) -> Result<Json<Notification>, AppError> {
    if body.workspace_id.trim().is_empty() {
        return Err(AppError::BadRequest("workspace_id is required".to_string()));
    }
    if body.kind != "webpush" && body.kind != "webhook" {
        return Err(AppError::BadRequest(
            "kind must be 'webpush' or 'webhook'".to_string(),
        ));
    }
    let n = state
        .notifications
        .create(
            &body.workspace_id,
            &body.kind,
            &body.config,
            body.label.as_deref(),
        )
        .await?;
    Ok(Json(n))
}

#[utoipa::path(
    delete,
    path = "/api/notifications/{id}",
    tag = "notifications",
    operation_id = "deleteNotification",
    params(("id" = String, Path, description = "Notification id")),
    responses((status = 204, description = "Deleted"))
)]
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    state.notifications.delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Result of sending a test notification: how many of the workspace's
/// notifications it attempted to deliver to.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct TestNotificationResult {
    pub delivered: usize,
}

#[utoipa::path(
    post,
    path = "/api/notifications/test",
    tag = "notifications",
    operation_id = "sendTestNotification",
    params(WorkspaceQuery),
    responses((status = 200, body = TestNotificationResult))
)]
pub async fn send_test(
    State(state): State<Arc<AppState>>,
    Query(q): Query<WorkspaceQuery>,
) -> Result<Json<TestNotificationResult>, AppError> {
    let notifications = state
        .notifications
        .list_by_workspace(&q.workspace_id)
        .await?;
    let mut delivered = 0usize;
    for n in &notifications {
        let result = match n.kind.as_str() {
            "webpush" => crate::notifier::webpush::deliver_test_to_notification(&state, n).await,
            "webhook" => deliver_test_webhook(&state, n).await,
            other => {
                tracing::warn!("test notification: unknown kind {}", other);
                continue;
            }
        };
        match result {
            Ok(()) => delivered += 1,
            Err(e) => tracing::warn!("test notification to {} ({}) failed: {:?}", n.kind, n.id, e),
        }
    }
    Ok(Json(TestNotificationResult { delivered }))
}

/// POST a fixed test payload to a webhook notification's URL.
async fn deliver_test_webhook(
    state: &Arc<AppState>,
    notification: &Notification,
) -> anyhow::Result<()> {
    let url = notification
        .config
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("webhook notification missing config.url"))?;
    let payload = serde_json::json!({
        "test": true,
        "message": "Pulp test notification — your webhook is reachable.",
    });
    let resp = state.http.post(url).json(&payload).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Webhook returned {}", resp.status());
    }
    Ok(())
}
