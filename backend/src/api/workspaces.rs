use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::db::repos::traits::Workspace;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct WorkspaceBody {
    pub name: String,
    pub description: Option<String>,
}

/// `GET /api/workspaces` — list all workspaces (not workspace-scoped).
#[utoipa::path(
    get,
    path = "/api/workspaces",
    tag = "workspaces",
    operation_id = "listWorkspaces",
    responses((status = 200, body = Vec<Workspace>))
)]
pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Vec<Workspace>>, AppError> {
    let workspaces = state.workspaces.list().await?;
    Ok(Json(workspaces))
}

/// `POST /api/workspaces` — create a workspace. Body: `{ name, description? }`.
#[utoipa::path(
    post,
    path = "/api/workspaces",
    tag = "workspaces",
    operation_id = "createWorkspace",
    request_body = WorkspaceBody,
    responses((status = 200, body = Workspace))
)]
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<WorkspaceBody>,
) -> Result<Json<Workspace>, AppError> {
    let workspace = state
        .workspaces
        .create(&body.name, body.description.as_deref())
        .await?;
    Ok(Json(workspace))
}

/// `PUT /api/workspaces/:id` — rename/update a workspace. Body: `{ name, description? }`.
#[utoipa::path(
    put,
    path = "/api/workspaces/{id}",
    tag = "workspaces",
    operation_id = "updateWorkspace",
    params(("id" = String, Path, description = "Workspace id")),
    request_body = WorkspaceBody,
    responses((status = 200, body = Workspace))
)]
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<WorkspaceBody>,
) -> Result<Json<Workspace>, AppError> {
    let workspace = state
        .workspaces
        .update(&id, &body.name, body.description.as_deref())
        .await?;
    Ok(Json(workspace))
}

/// `DELETE /api/workspaces/:id` — delete a workspace (cascades to its monitors and notifications).
#[utoipa::path(
    delete,
    path = "/api/workspaces/{id}",
    tag = "workspaces",
    operation_id = "deleteWorkspace",
    params(("id" = String, Path, description = "Workspace id")),
    responses((status = 204, description = "Deleted"))
)]
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    state.workspaces.delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}
