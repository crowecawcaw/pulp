use async_trait::async_trait;
use sqlx::SqlitePool;
use ulid::Ulid;

use crate::db::repos::traits::{Workspace, WorkspaceRepo};
use crate::error::AppError;

#[derive(sqlx::FromRow)]
struct WorkspaceRow {
    id: String,
    name: String,
    description: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl From<WorkspaceRow> for Workspace {
    fn from(r: WorkspaceRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            description: r.description,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub struct SqliteWorkspaceRepo {
    pool: SqlitePool,
}

impl SqliteWorkspaceRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl WorkspaceRepo for SqliteWorkspaceRepo {
    async fn list(&self) -> Result<Vec<Workspace>, AppError> {
        let rows = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, name, description, created_at, updated_at FROM workspaces ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Workspace::from).collect())
    }

    async fn get(&self, id: &str) -> Result<Option<Workspace>, AppError> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, name, description, created_at, updated_at FROM workspaces WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Workspace::from))
    }

    async fn create(&self, name: &str, description: Option<&str>) -> Result<Workspace, AppError> {
        let id = Ulid::new().to_string();
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO workspaces (id, name, description, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(description)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(Workspace {
            id,
            name: name.to_string(),
            description: description.map(|s| s.to_string()),
            created_at: now,
            updated_at: now,
        })
    }

    async fn update(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<Workspace, AppError> {
        let now = chrono::Utc::now().timestamp();
        let affected = sqlx::query(
            "UPDATE workspaces SET name = ?, description = ?, updated_at = ? WHERE id = ?",
        )
        .bind(name)
        .bind(description)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound);
        }

        self.get(id).await?.ok_or(AppError::NotFound)
    }

    async fn delete(&self, id: &str) -> Result<(), AppError> {
        let affected = sqlx::query("DELETE FROM workspaces WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    }
}
