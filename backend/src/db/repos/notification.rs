use async_trait::async_trait;
use sqlx::SqlitePool;
use ulid::Ulid;

use crate::db::repos::traits::{Notification, NotificationRepo};
use crate::error::AppError;

#[derive(sqlx::FromRow)]
struct NotificationRow {
    id: String,
    workspace_id: String,
    kind: String,
    config: String,
    label: Option<String>,
    created_at: i64,
}

impl From<NotificationRow> for Notification {
    fn from(r: NotificationRow) -> Self {
        Self {
            id: r.id,
            workspace_id: r.workspace_id,
            kind: r.kind,
            config: serde_json::from_str(&r.config)
                .unwrap_or(serde_json::Value::Object(Default::default())),
            label: r.label,
            created_at: r.created_at,
        }
    }
}

pub struct SqliteNotificationRepo {
    pool: SqlitePool,
}

impl SqliteNotificationRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn get_row(&self, id: &str) -> Result<Option<NotificationRow>, AppError> {
        let row = sqlx::query_as::<_, NotificationRow>(
            "SELECT id, workspace_id, kind, config, label, created_at FROM notifications WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }
}

#[async_trait]
impl NotificationRepo for SqliteNotificationRepo {
    async fn list_by_workspace(&self, workspace_id: &str) -> Result<Vec<Notification>, AppError> {
        let rows = sqlx::query_as::<_, NotificationRow>(
            "SELECT id, workspace_id, kind, config, label, created_at FROM notifications \
             WHERE workspace_id = ? ORDER BY created_at ASC",
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Notification::from).collect())
    }

    async fn create(
        &self,
        workspace_id: &str,
        kind: &str,
        config: &serde_json::Value,
        label: Option<&str>,
    ) -> Result<Notification, AppError> {
        let id = Ulid::new().to_string();
        let now = chrono::Utc::now().timestamp();
        let config_json = serde_json::to_string(config).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO notifications (id, workspace_id, kind, config, label, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(workspace_id)
        .bind(kind)
        .bind(&config_json)
        .bind(label)
        .bind(now)
        .execute(&self.pool)
        .await?;

        self.get(&id).await?.ok_or(AppError::NotFound)
    }

    async fn get(&self, id: &str) -> Result<Option<Notification>, AppError> {
        Ok(self.get_row(id).await?.map(Notification::from))
    }

    async fn delete(&self, id: &str) -> Result<(), AppError> {
        let affected = sqlx::query("DELETE FROM notifications WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    async fn delete_by_endpoint(&self, endpoint: &str) -> Result<u64, AppError> {
        // Only webpush notifications carry an endpoint; match it inside the
        // config JSON via SQLite's json_extract.
        let affected = sqlx::query(
            "DELETE FROM notifications \
             WHERE kind = 'webpush' AND json_extract(config, '$.endpoint') = ?",
        )
        .bind(endpoint)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(affected)
    }
}
