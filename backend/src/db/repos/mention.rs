use async_trait::async_trait;
use sqlx::SqlitePool;
use ulid::Ulid;

use crate::db::repos::traits::{
    Mention, MentionFilter, MentionRepo, NewMention, PendingCount, PendingNotification,
};
use crate::error::AppError;

#[derive(sqlx::FromRow)]
struct MentionRow {
    id: String,
    monitor_id: String,
    channel: String,
    external_id: String,
    content_text: String,
    content_url: String,
    author_name: Option<String>,
    author_url: Option<String>,
    published_at: Option<i64>,
    ingested_at: i64,
    platform_meta: String,
    read_at: Option<i64>,
    ai_verdict: Option<String>,
    ai_reason: Option<String>,
}

/// A mention row joined to its owning workspace id (via monitor), for the
/// notifier fan-out query. `flatten` reuses `MentionRow`'s columns.
#[derive(sqlx::FromRow)]
struct UnnotifiedRow {
    #[sqlx(flatten)]
    mention: MentionRow,
    workspace_id: String,
}

impl From<MentionRow> for Mention {
    fn from(r: MentionRow) -> Self {
        Self {
            id: r.id,
            monitor_id: r.monitor_id,
            channel: r.channel,
            external_id: r.external_id,
            content_text: r.content_text,
            content_url: r.content_url,
            author_name: r.author_name,
            author_url: r.author_url,
            published_at: r.published_at,
            ingested_at: r.ingested_at,
            platform_meta: serde_json::from_str(&r.platform_meta)
                .unwrap_or(serde_json::Value::Object(Default::default())),
            read_at: r.read_at,
            ai_verdict: r.ai_verdict,
            ai_reason: r.ai_reason,
        }
    }
}

pub struct SqliteMentionRepo {
    pool: SqlitePool,
}

impl SqliteMentionRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MentionRepo for SqliteMentionRepo {
    async fn list(&self, filter: MentionFilter) -> Result<(Vec<Mention>, bool), AppError> {
        // SQLite treats a negative LIMIT as "unlimited", so clamp the lower
        // bound too, not just the upper one.
        let limit = filter.limit.unwrap_or(50).clamp(1, 200);
        // Fetch one extra to detect has_more
        let fetch_limit = limit + 1;

        // Build query dynamically
        let mut conditions: Vec<String> = vec![];
        let mut bind_values: Vec<Option<String>> = vec![];

        if let Some(ref ws_id) = filter.workspace_id {
            conditions.push(
                "m.monitor_id IN (SELECT id FROM monitors WHERE workspace_id = ?)".to_string(),
            );
            bind_values.push(Some(ws_id.clone()));
        }

        if let Some(ref ch) = filter.channel {
            conditions.push("m.channel = ?".to_string());
            bind_values.push(Some(ch.clone()));
        }

        if let Some(ref kid) = filter.monitor_id {
            conditions.push("m.monitor_id = ?".to_string());
            bind_values.push(Some(kid.clone()));
        }

        // Compound keyset cursor: `published_at` is nullable and non-unique,
        // so a bare `published_at < before` both drops NULL-published rows
        // and skips/duplicates rows that tie on `published_at` across a page
        // boundary. Instead we order by a never-null effective timestamp
        // (`COALESCE(published_at, ingested_at)`) with `id` as a tiebreaker.
        // `before` alone is a plain upper bound on that effective timestamp
        // (e.g. an ad-hoc "mentions before time X" query). When `before_id`
        // is also given — the shape the feed's "load more" sends — it adds
        // the tiebreak so paging one row at a time can't skip or duplicate a
        // row that ties with the cursor's effective timestamp: strictly-less
        // on effective_ts, OR tied on effective_ts and strictly-less on id.
        // `COALESCE(...)` is a function call, not a bare column reference, so
        // unlike `m.published_at < ?` SQLite does NOT apply the underlying
        // columns' INTEGER affinity to the bound parameter here — a
        // TEXT-typed bind would compare as TEXT (and SQLite's type-sorting
        // rules put every INTEGER before every TEXT, so it would match
        // everything). `CAST(? AS INTEGER)` forces the numeric comparison
        // the timestamps need.
        if let Some(before) = filter.before {
            match filter.before_id.as_ref() {
                Some(before_id) => {
                    conditions.push(
                        "(COALESCE(m.published_at, m.ingested_at) < CAST(? AS INTEGER) \
                          OR (COALESCE(m.published_at, m.ingested_at) = CAST(? AS INTEGER) AND m.id < ?))"
                            .to_string(),
                    );
                    bind_values.push(Some(before.to_string()));
                    bind_values.push(Some(before.to_string()));
                    bind_values.push(Some(before_id.clone()));
                }
                None => {
                    conditions.push(
                        "COALESCE(m.published_at, m.ingested_at) < CAST(? AS INTEGER)".to_string(),
                    );
                    bind_values.push(Some(before.to_string()));
                }
            }
        }

        if let Some(since) = filter.since {
            conditions.push("m.published_at >= ?".to_string());
            bind_values.push(Some(since.to_string()));
        }

        match filter.read {
            Some(true) => conditions.push("m.read_at IS NOT NULL".to_string()),
            Some(false) => conditions.push("m.read_at IS NULL".to_string()),
            None => {}
        }

        if let Some(ref verdict) = filter.ai_verdict {
            conditions.push("m.ai_verdict = ?".to_string());
            bind_values.push(Some(verdict.clone()));
        }

        if filter.ai_visible_only {
            conditions.push("(m.ai_verdict IS NULL OR m.ai_verdict = 'accepted')".to_string());
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT m.id, m.monitor_id, m.channel, m.external_id, m.content_text, m.content_url, m.author_name, m.author_url, m.published_at, m.ingested_at, m.platform_meta, m.read_at, m.ai_verdict, m.ai_reason FROM mentions m {} ORDER BY COALESCE(m.published_at, m.ingested_at) DESC, m.id DESC LIMIT ?",
            where_clause
        );

        let mut query = sqlx::query_as::<_, MentionRow>(&sql);

        for val in &bind_values {
            match val {
                Some(v) => {
                    query = query.bind(v.clone());
                }
                None => {
                    query = query.bind(Option::<String>::None);
                }
            }
        }

        // Timestamp bounds are bound as strings above; SQLite applies the
        // column's INTEGER affinity to coerce them for the comparison.
        query = query.bind(fetch_limit);

        let mut rows = query.fetch_all(&self.pool).await?;

        let has_more = rows.len() as i64 > limit;
        if has_more {
            rows.truncate(limit as usize);
        }

        Ok((rows.into_iter().map(Mention::from).collect(), has_more))
    }

    async fn get(&self, id: &str) -> Result<Option<Mention>, AppError> {
        let row = sqlx::query_as::<_, MentionRow>(
            "SELECT id, monitor_id, channel, external_id, content_text, content_url, author_name, author_url, published_at, ingested_at, platform_meta, read_at, ai_verdict, ai_reason FROM mentions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Mention::from))
    }

    async fn exists(
        &self,
        monitor_id: &str,
        channel: &str,
        external_id: &str,
    ) -> Result<bool, AppError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM mentions WHERE monitor_id = ? AND channel = ? AND external_id = ?",
        )
        .bind(monitor_id)
        .bind(channel)
        .bind(external_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0 > 0)
    }

    async fn insert(&self, new: NewMention) -> Result<Mention, AppError> {
        let id = Ulid::new().to_string();
        let now = chrono::Utc::now().timestamp();
        let platform_meta_json =
            serde_json::to_string(&new.platform_meta).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO mentions (id, monitor_id, channel, external_id, content_text, content_url, author_name, author_url, published_at, ingested_at, platform_meta, ai_verdict) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&new.monitor_id)
        .bind(&new.channel)
        .bind(&new.external_id)
        .bind(&new.content_text)
        .bind(&new.content_url)
        .bind(&new.author_name)
        .bind(&new.author_url)
        .bind(new.published_at)
        .bind(now)
        .bind(&platform_meta_json)
        .bind(&new.ai_verdict)
        .execute(&self.pool)
        .await?;

        let row = sqlx::query_as::<_, MentionRow>(
            "SELECT id, monitor_id, channel, external_id, content_text, content_url, author_name, author_url, published_at, ingested_at, platform_meta, read_at, ai_verdict, ai_reason FROM mentions WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&self.pool)
        .await?;

        Ok(Mention::from(row))
    }

    async fn set_read(&self, id: &str, read: bool) -> Result<Mention, AppError> {
        let read_at = if read {
            Some(chrono::Utc::now().timestamp())
        } else {
            None
        };
        let result = sqlx::query("UPDATE mentions SET read_at = ? WHERE id = ?")
            .bind(read_at)
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }

        let row = sqlx::query_as::<_, MentionRow>(
            "SELECT id, monitor_id, channel, external_id, content_text, content_url, author_name, author_url, published_at, ingested_at, platform_meta, read_at, ai_verdict, ai_reason FROM mentions WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(Mention::from(row))
    }

    async fn list_ai_pending(&self, limit: i64) -> Result<Vec<Mention>, AppError> {
        let rows = sqlx::query_as::<_, MentionRow>(
            "SELECT id, monitor_id, channel, external_id, content_text, content_url, \
             author_name, author_url, published_at, ingested_at, platform_meta, \
             read_at, ai_verdict, ai_reason \
             FROM mentions WHERE ai_verdict = 'pending' ORDER BY ingested_at ASC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Mention::from).collect())
    }

    async fn count_ai_pending(&self, workspace_id: Option<&str>) -> Result<PendingCount, AppError> {
        // COUNT + MIN(ingested_at) in one pass. When scoped, restrict to the
        // workspace's monitors (same subquery shape `list` uses).
        let (count, oldest_ingested_at) = if let Some(ws) = workspace_id {
            sqlx::query_as::<_, (i64, Option<i64>)>(
                "SELECT COUNT(*), MIN(ingested_at) FROM mentions \
                 WHERE ai_verdict = 'pending' \
                   AND monitor_id IN (SELECT id FROM monitors WHERE workspace_id = ?)",
            )
            .bind(ws)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, (i64, Option<i64>)>(
                "SELECT COUNT(*), MIN(ingested_at) FROM mentions WHERE ai_verdict = 'pending'",
            )
            .fetch_one(&self.pool)
            .await?
        };
        Ok(PendingCount {
            count,
            oldest_ingested_at,
        })
    }

    async fn set_ai_verdict(
        &self,
        id: &str,
        verdict: &str,
        reason: Option<&str>,
    ) -> Result<Mention, AppError> {
        let result = sqlx::query("UPDATE mentions SET ai_verdict = ?, ai_reason = ? WHERE id = ?")
            .bind(verdict)
            .bind(reason)
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }

        let row = sqlx::query_as::<_, MentionRow>(
            "SELECT id, monitor_id, channel, external_id, content_text, content_url, author_name, author_url, published_at, ingested_at, platform_meta, read_at, ai_verdict, ai_reason FROM mentions WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(Mention::from(row))
    }

    async fn bump_ai_attempts(&self, id: &str) -> Result<i64, AppError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "UPDATE mentions SET ai_attempts = ai_attempts + 1 WHERE id = ? \
             RETURNING ai_attempts",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn list_for_channel(&self, channel: &str) -> Result<Vec<Mention>, AppError> {
        let rows = sqlx::query_as::<_, MentionRow>(
            "SELECT id, monitor_id, channel, external_id, content_text, content_url, \
             author_name, author_url, published_at, ingested_at, platform_meta, \
             read_at, ai_verdict, ai_reason \
             FROM mentions WHERE channel = ? \
             ORDER BY ingested_at DESC",
        )
        .bind(channel)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Mention::from).collect())
    }

    async fn delete_many(&self, ids: &[String]) -> Result<u64, AppError> {
        if ids.is_empty() {
            return Ok(0);
        }

        let mut tx = self.pool.begin().await?;
        let mut total: u64 = 0;

        for id in ids {
            let result = sqlx::query("DELETE FROM mentions WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            total += result.rows_affected();
        }

        tx.commit().await?;
        Ok(total)
    }

    async fn list_unnotified(&self, limit: i64) -> Result<Vec<PendingNotification>, AppError> {
        // Feed-visible (ai_verdict NULL or 'accepted') AND not yet notified,
        // joined to the owning workspace via the mention's monitor. Oldest
        // first so a bounded pass drains the backlog deterministically.
        let rows = sqlx::query_as::<_, UnnotifiedRow>(
            "SELECT m.id, m.monitor_id, m.channel, m.external_id, m.content_text, m.content_url, \
             m.author_name, m.author_url, m.published_at, m.ingested_at, \
             m.platform_meta, m.read_at, m.ai_verdict, m.ai_reason, mo.workspace_id \
             FROM mentions m JOIN monitors mo ON mo.id = m.monitor_id \
             WHERE m.notified_at IS NULL \
               AND (m.ai_verdict IS NULL OR m.ai_verdict = 'accepted') \
             ORDER BY m.ingested_at ASC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let workspace_id = r.workspace_id.clone();
                PendingNotification {
                    mention: Mention::from(r.mention),
                    workspace_id,
                }
            })
            .collect())
    }

    async fn mark_notified(&self, ids: &[String]) -> Result<u64, AppError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let now = chrono::Utc::now().timestamp();
        let mut tx = self.pool.begin().await?;
        let mut total: u64 = 0;
        for id in ids {
            let result = sqlx::query("UPDATE mentions SET notified_at = ? WHERE id = ?")
                .bind(now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            total += result.rows_affected();
        }
        tx.commit().await?;
        Ok(total)
    }
}
