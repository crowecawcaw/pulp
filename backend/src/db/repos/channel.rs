use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::db::repos::traits::{ChannelConfig, ChannelRepo};
use crate::error::AppError;

#[derive(sqlx::FromRow)]
struct ChannelConfigRow {
    channel: String,
    enabled: i64,
    credentials: String,
    poll_interval: i64,
    last_polled_at: Option<i64>,
    error_message: Option<String>,
    updated_at: i64,
    caught_up_at: Option<i64>,
    max_backfill_days: i64,
}

impl From<ChannelConfigRow> for ChannelConfig {
    fn from(r: ChannelConfigRow) -> Self {
        Self {
            channel: r.channel,
            enabled: r.enabled != 0,
            credentials: serde_json::from_str(&r.credentials)
                .unwrap_or(serde_json::Value::Object(Default::default())),
            poll_interval: r.poll_interval,
            last_polled_at: r.last_polled_at,
            error_message: r.error_message,
            updated_at: r.updated_at,
            caught_up_at: r.caught_up_at,
            max_backfill_days: r.max_backfill_days,
        }
    }
}

pub struct SqliteChannelRepo {
    pool: SqlitePool,
}

impl SqliteChannelRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ChannelRepo for SqliteChannelRepo {
    async fn list(&self) -> Result<Vec<ChannelConfig>, AppError> {
        let rows = sqlx::query_as::<_, ChannelConfigRow>(
            "SELECT channel, enabled, credentials, poll_interval, last_polled_at, error_message, updated_at, caught_up_at, max_backfill_days FROM channel_configs ORDER BY channel ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(ChannelConfig::from).collect())
    }

    async fn get(&self, channel: &str) -> Result<Option<ChannelConfig>, AppError> {
        let row = sqlx::query_as::<_, ChannelConfigRow>(
            "SELECT channel, enabled, credentials, poll_interval, last_polled_at, error_message, updated_at, caught_up_at, max_backfill_days FROM channel_configs WHERE channel = ?",
        )
        .bind(channel)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(ChannelConfig::from))
    }

    async fn upsert(
        &self,
        channel: &str,
        enabled: bool,
        credentials: Option<serde_json::Value>,
        poll_interval: i64,
    ) -> Result<ChannelConfig, AppError> {
        let now = chrono::Utc::now().timestamp();
        let enabled_i = enabled as i64;

        match credentials {
            Some(creds) => {
                // Explicit credentials: overwrite them like before.
                let credentials_json =
                    serde_json::to_string(&creds).unwrap_or_else(|_| "{}".to_string());
                sqlx::query(
                    "INSERT INTO channel_configs (channel, enabled, credentials, poll_interval, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(channel) DO UPDATE SET enabled = excluded.enabled, credentials = excluded.credentials, poll_interval = excluded.poll_interval, updated_at = excluded.updated_at",
                )
                .bind(channel)
                .bind(enabled_i)
                .bind(&credentials_json)
                .bind(poll_interval)
                .bind(now)
                .execute(&self.pool)
                .await?;
            }
            None => {
                // No credentials provided: leave the stored value untouched on
                // conflict (the column is simply absent from the SET clause).
                // A brand-new row still needs *some* value, so seed it with `{}`.
                sqlx::query(
                    "INSERT INTO channel_configs (channel, enabled, credentials, poll_interval, updated_at) VALUES (?, ?, '{}', ?, ?) ON CONFLICT(channel) DO UPDATE SET enabled = excluded.enabled, poll_interval = excluded.poll_interval, updated_at = excluded.updated_at",
                )
                .bind(channel)
                .bind(enabled_i)
                .bind(poll_interval)
                .bind(now)
                .execute(&self.pool)
                .await?;
            }
        }

        self.get(channel).await?.ok_or(AppError::NotFound)
    }

    async fn update_polled(
        &self,
        channel: &str,
        error_message: Option<&str>,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "UPDATE channel_configs SET last_polled_at = ?, error_message = ?, updated_at = ? WHERE channel = ?",
        )
        .bind(now)
        .bind(error_message)
        .bind(now)
        .bind(channel)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn set_caught_up_now(&self, channel: &str) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query("UPDATE channel_configs SET caught_up_at = ? WHERE channel = ?")
            .bind(now)
            .bind(channel)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_caught_up_at(&self, channel: &str, value: Option<i64>) -> Result<(), AppError> {
        let Some(value) = value else { return Ok(()) };
        sqlx::query("UPDATE channel_configs SET caught_up_at = ? WHERE channel = ?")
            .bind(value)
            .bind(channel)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
