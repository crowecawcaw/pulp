use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::IntoParams;

use crate::db::repos::traits::{CreateMonitor, Monitor, UpdateMonitor};
use crate::error::AppError;
use crate::state::AppState;

#[derive(Deserialize, IntoParams)]
pub struct ListQuery {
    pub workspace_id: String,
}

/// `GET /api/monitors?workspace_id=…` — list monitors in a workspace.
/// The `workspace_id` query param is required.
#[utoipa::path(
    get,
    path = "/api/monitors",
    tag = "monitors",
    operation_id = "listMonitors",
    params(ListQuery),
    responses((status = 200, body = Vec<Monitor>))
)]
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Monitor>>, AppError> {
    let monitors = state.monitors.list(&q.workspace_id).await?;
    Ok(Json(monitors))
}

/// `GET /api/monitors/:id` — fetch a single monitor.
#[utoipa::path(
    get,
    path = "/api/monitors/{id}",
    tag = "monitors",
    operation_id = "getMonitor",
    params(("id" = String, Path, description = "Monitor id")),
    responses(
        (status = 200, body = Monitor),
        (status = 404, description = "Monitor not found")
    )
)]
pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Monitor>, AppError> {
    let monitor = state
        .monitors
        .get(&id)
        .await?
        .ok_or(crate::error::AppError::NotFound)?;
    Ok(Json(monitor))
}

/// `POST /api/monitors` — create a monitor watch. Body is [`CreateMonitor`]:
/// `workspace_id` and `terms` (the match-any keyword list) are required;
/// `channels` (empty/omitted = all channels), `exact_match`, `case_sensitive`,
/// `exclude_terms` are optional.
#[utoipa::path(
    post,
    path = "/api/monitors",
    tag = "monitors",
    operation_id = "createMonitor",
    request_body = CreateMonitor,
    responses((status = 200, body = Monitor))
)]
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateMonitor>,
) -> Result<Json<Monitor>, AppError> {
    let monitor = state.monitors.create(body).await?;
    Ok(Json(monitor))
}

/// `PUT /api/monitors/:id` — update a monitor watch. Body is [`UpdateMonitor`];
/// every field is optional and only provided fields are changed.
#[utoipa::path(
    put,
    path = "/api/monitors/{id}",
    tag = "monitors",
    operation_id = "updateMonitor",
    params(("id" = String, Path, description = "Monitor id")),
    request_body = UpdateMonitor,
    responses((status = 200, body = Monitor))
)]
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateMonitor>,
) -> Result<Json<Monitor>, AppError> {
    let monitor = state.monitors.update(&id, body).await?;
    Ok(Json(monitor))
}

/// `DELETE /api/monitors/:id` — delete a monitor watch (cascades to its mentions).
#[utoipa::path(
    delete,
    path = "/api/monitors/{id}",
    tag = "monitors",
    operation_id = "deleteMonitor",
    params(("id" = String, Path, description = "Monitor id")),
    responses((status = 204, description = "Deleted"))
)]
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    state.monitors.delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}
