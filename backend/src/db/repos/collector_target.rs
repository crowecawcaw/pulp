use async_trait::async_trait;
use sqlx::SqlitePool;
use ulid::Ulid;

use crate::db::repos::traits::{BackfillJob, CollectorTarget, CollectorTargetRepo};
use crate::error::AppError;

/// Deterministic, stable target id from its identity tuple. Re-planning the same
/// (channel, kind, descriptor) yields the same id, so `upsert_target` is
/// idempotent and progress/status survives across passes. Hex of a 64-bit FNV-1a
/// hash keeps it short, opaque, and SQL-safe.
pub fn target_id(channel: &str, kind: &str, descriptor: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for part in [channel, kind, descriptor] {
        for b in part.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        // Separator so ("a","b") and ("ab","") can't collide.
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("tgt_{:016x}", hash)
}

#[derive(sqlx::FromRow)]
struct TargetRow {
    id: String,
    channel: String,
    kind: String,
    descriptor: String,
    confirmed_watermark: Option<i64>,
    last_success_at: Option<i64>,
    last_attempt_at: Option<i64>,
    consecutive_failures: i64,
    last_error: Option<String>,
    updated_at: i64,
}

impl From<TargetRow> for CollectorTarget {
    fn from(r: TargetRow) -> Self {
        Self {
            id: r.id,
            channel: r.channel,
            kind: r.kind,
            descriptor: r.descriptor,
            confirmed_watermark: r.confirmed_watermark,
            last_success_at: r.last_success_at,
            last_attempt_at: r.last_attempt_at,
            consecutive_failures: r.consecutive_failures,
            last_error: r.last_error,
            updated_at: r.updated_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct JobRow {
    id: String,
    target_id: String,
    range_start: i64,
    range_end: i64,
    next_cursor: Option<String>,
    state: String,
    pages_done: i64,
    attempts: i64,
    last_error: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl From<JobRow> for BackfillJob {
    fn from(r: JobRow) -> Self {
        Self {
            id: r.id,
            target_id: r.target_id,
            range_start: r.range_start,
            range_end: r.range_end,
            next_cursor: r.next_cursor,
            state: r.state,
            pages_done: r.pages_done,
            attempts: r.attempts,
            last_error: r.last_error,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

const TARGET_COLS: &str = "id, channel, kind, descriptor, confirmed_watermark, last_success_at, last_attempt_at, consecutive_failures, last_error, updated_at";
const JOB_COLS: &str = "id, target_id, range_start, range_end, next_cursor, state, pages_done, attempts, last_error, created_at, updated_at";

pub struct SqliteCollectorTargetRepo {
    pool: SqlitePool,
}

impl SqliteCollectorTargetRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CollectorTargetRepo for SqliteCollectorTargetRepo {
    async fn upsert_target(
        &self,
        channel: &str,
        kind: &str,
        descriptor: &str,
    ) -> Result<CollectorTarget, AppError> {
        let id = target_id(channel, kind, descriptor);
        let now = chrono::Utc::now().timestamp();
        // Insert if new; on conflict only refresh the descriptor (it's derived
        // from the same inputs) + updated_at, and CLEAR retired_at (re-planning a
        // target un-retires it, resuming from its preserved watermark). NEVER
        // touch status columns here — that's what record_target_*; do. This keeps
        // re-planning idempotent.
        sqlx::query(
            "INSERT INTO collector_targets (id, channel, kind, descriptor, consecutive_failures, updated_at) \
             VALUES (?, ?, ?, ?, 0, ?) \
             ON CONFLICT(id) DO UPDATE SET descriptor = excluded.descriptor, updated_at = excluded.updated_at, retired_at = NULL",
        )
        .bind(&id)
        .bind(channel)
        .bind(kind)
        .bind(descriptor)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_target(&id).await?.ok_or(AppError::NotFound)
    }

    async fn get_target(&self, id: &str) -> Result<Option<CollectorTarget>, AppError> {
        let row = sqlx::query_as::<_, TargetRow>(&format!(
            "SELECT {TARGET_COLS} FROM collector_targets WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(CollectorTarget::from))
    }

    async fn list_targets(&self, channel: &str) -> Result<Vec<CollectorTarget>, AppError> {
        let rows = sqlx::query_as::<_, TargetRow>(&format!(
            "SELECT {TARGET_COLS} FROM collector_targets \
             WHERE channel = ? AND retired_at IS NULL ORDER BY descriptor ASC"
        ))
        .bind(channel)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(CollectorTarget::from).collect())
    }

    async fn set_target_members(
        &self,
        channel: &str,
        target_id: &str,
        monitor_ids: &[String],
    ) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM target_monitors WHERE target_id = ?")
            .bind(target_id)
            .execute(&mut *tx)
            .await?;
        for mid in monitor_ids {
            // OR IGNORE: tolerate a monitor deleted mid-pass (FK violation) and
            // duplicate ids without failing the whole membership write.
            sqlx::query(
                "INSERT OR IGNORE INTO target_monitors (target_id, monitor_id, channel) \
                 VALUES (?, ?, ?)",
            )
            .bind(target_id)
            .bind(mid)
            .bind(channel)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn reconcile_targets(&self, channel: &str, live_ids: &[String]) -> Result<u64, AppError> {
        let now = chrono::Utc::now().timestamp();
        let mut sql = String::from(
            "UPDATE collector_targets SET retired_at = ? \
             WHERE channel = ? AND retired_at IS NULL",
        );
        if !live_ids.is_empty() {
            sql.push_str(" AND id NOT IN (");
            sql.push_str(&vec!["?"; live_ids.len()].join(","));
            sql.push(')');
        }
        let mut q = sqlx::query(&sql).bind(now).bind(channel);
        for id in live_ids {
            q = q.bind(id);
        }
        let res = q.execute(&self.pool).await?;
        Ok(res.rows_affected())
    }

    async fn purge_retired(&self, older_than: i64) -> Result<u64, AppError> {
        let res = sqlx::query(
            "DELETE FROM collector_targets WHERE retired_at IS NOT NULL AND retired_at < ?",
        )
        .bind(older_than)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    async fn record_target_success(
        &self,
        id: &str,
        confirmed_watermark: Option<i64>,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        // Persist whatever watermark the caller computed, verbatim, when one is
        // supplied; `None` leaves the stored value untouched (a head fetch that
        // didn't reach the watermark passes None to avoid falsely marking
        // "caught up"). This is NOT a monotonic-decrease-only clamp: the single
        // caller, `scheduler::advance_watermark`, already owns the full
        // contiguity logic — including the deliberate exception where it moves
        // the watermark FORWARD (to a newer/larger value) when a head walk
        // exhausts the upstream source without ever reaching the prior
        // watermark, because the older anchor has aged out of the source's
        // search horizon and paging further can never prove contiguity down to
        // it. A second, conflicting "only ever get older" clamp here would
        // silently discard that unwedging move and leave the watermark stuck
        // forever, so this method just trusts and stores the supplied value.
        //
        // All placeholders are explicitly numbered: mixing anonymous `?` with
        // `?N` numbers the anonymous ones first (left-to-right), which silently
        // bound `last_attempt_at` to the watermark instead of `now` — making a
        // healthy target look like it hadn't been attempted in months.
        sqlx::query(
            "UPDATE collector_targets SET \
               last_success_at = ?1, last_attempt_at = ?1, \
               consecutive_failures = 0, last_error = NULL, \
               confirmed_watermark = CASE \
                 WHEN ?2 IS NULL THEN confirmed_watermark \
                 ELSE ?2 END, \
               updated_at = ?1 \
             WHERE id = ?3",
        )
        .bind(now)
        .bind(confirmed_watermark)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_target_failure(&self, id: &str, error: &str) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "UPDATE collector_targets SET \
               last_attempt_at = ?, consecutive_failures = consecutive_failures + 1, \
               last_error = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(now)
        .bind(error)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn enqueue_job(
        &self,
        target_id: &str,
        range_start: i64,
        range_end: i64,
    ) -> Result<BackfillJob, AppError> {
        // Coalesce: if an open job for this target already covers (overlaps) the
        // requested window, return it rather than piling up duplicates.
        if let Some(existing) = sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {JOB_COLS} FROM collector_backfill_jobs \
             WHERE target_id = ? AND state = 'open' \
               AND range_start <= ? AND range_end >= ? \
             ORDER BY created_at ASC LIMIT 1"
        ))
        .bind(target_id)
        .bind(range_end)
        .bind(range_start)
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(existing.into());
        }

        let id = format!("job_{}", Ulid::new());
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO collector_backfill_jobs \
               (id, target_id, range_start, range_end, state, pages_done, attempts, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'open', 0, 0, ?, ?)",
        )
        .bind(&id)
        .bind(target_id)
        .bind(range_start)
        .bind(range_end)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_job(&id).await?.ok_or(AppError::NotFound)
    }

    async fn get_job(&self, id: &str) -> Result<Option<BackfillJob>, AppError> {
        let row = sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {JOB_COLS} FROM collector_backfill_jobs WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(BackfillJob::from))
    }

    async fn list_open_jobs_for_channel(
        &self,
        channel: &str,
    ) -> Result<Vec<BackfillJob>, AppError> {
        let rows = sqlx::query_as::<_, JobRow>(
            "SELECT j.id, j.target_id, j.range_start, j.range_end, j.next_cursor, j.state, \
                    j.pages_done, j.attempts, j.last_error, j.created_at, j.updated_at \
             FROM collector_backfill_jobs j \
             JOIN collector_targets t ON t.id = j.target_id \
             WHERE t.channel = ? AND j.state = 'open' \
             ORDER BY j.created_at ASC",
        )
        .bind(channel)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(BackfillJob::from).collect())
    }

    async fn list_open_jobs_for_target(
        &self,
        target_id: &str,
    ) -> Result<Vec<BackfillJob>, AppError> {
        let rows = sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {JOB_COLS} FROM collector_backfill_jobs \
             WHERE target_id = ? AND state = 'open' ORDER BY created_at ASC"
        ))
        .bind(target_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(BackfillJob::from).collect())
    }

    async fn update_job_progress(
        &self,
        id: &str,
        next_cursor: Option<&str>,
        pages_done: i64,
        range_end: i64,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "UPDATE collector_backfill_jobs SET \
               next_cursor = ?, pages_done = ?, range_end = ?, \
               attempts = attempts + 1, updated_at = ? \
             WHERE id = ?",
        )
        .bind(next_cursor)
        .bind(pages_done)
        .bind(range_end)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn complete_job(&self, id: &str) -> Result<(), AppError> {
        self.mark_job(id, "done", None).await
    }

    async fn mark_job(&self, id: &str, state: &str, error: Option<&str>) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "UPDATE collector_backfill_jobs SET state = ?, last_error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(state)
        .bind(error)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_id_is_stable_and_distinct() {
        let a = target_id("reddit", "search", "\"x\" OR \"y\"");
        let b = target_id("reddit", "search", "\"x\" OR \"y\"");
        assert_eq!(a, b, "same inputs → same id");
        assert_ne!(a, target_id("reddit", "feed", "\"x\" OR \"y\""));
        assert_ne!(a, target_id("reddit", "search", "\"x\""));
        assert_ne!(a, target_id("hackernews", "search", "\"x\" OR \"y\""));
        // No field-boundary collisions.
        assert_ne!(
            target_id("reddit", "feed", "a"),
            target_id("reddit", "fee", "da")
        );
        assert!(a.starts_with("tgt_"));
    }
}
