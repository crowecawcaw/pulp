use async_trait::async_trait;
use sqlx::SqlitePool;
use ulid::Ulid;

use crate::db::repos::traits::{CreateMonitor, Monitor, MonitorRepo, UpdateMonitor};
use crate::error::AppError;

#[derive(sqlx::FromRow)]
struct MonitorRow {
    id: String,
    workspace_id: String,
    terms: String,
    active: i64,
    channels: String,
    exact_match: i64,
    case_sensitive: i64,
    exclude_terms: String,
    channel_settings: String,
    ai_filter_prompt: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl From<MonitorRow> for Monitor {
    fn from(r: MonitorRow) -> Self {
        Self {
            id: r.id,
            workspace_id: r.workspace_id,
            terms: serde_json::from_str(&r.terms).unwrap_or_default(),
            active: r.active != 0,
            channels: serde_json::from_str(&r.channels).unwrap_or_default(),
            exact_match: r.exact_match != 0,
            case_sensitive: r.case_sensitive != 0,
            exclude_terms: serde_json::from_str(&r.exclude_terms).unwrap_or_default(),
            channel_settings: serde_json::from_str(&r.channel_settings)
                .unwrap_or(serde_json::Value::Object(Default::default())),
            ai_filter_prompt: r.ai_filter_prompt,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Split a legacy single `phrase` into the explicit term list it should have
/// become. Pure (no I/O) so the auto-repair logic is unit-tested.
///
/// The old model stored OR-phrases like `"nimbusdb" OR "self-hosted analytics"` in a
/// single `phrase` that the fuzzy matcher then never matched (it looked for the
/// literal substring including the quotes/OR). The new model is one bare literal
/// per term, OR-combined across terms. So we split on a top-level ` OR `
/// (case-insensitive, surrounded by spaces), trim each piece, strip a single
/// pair of wrapping double-quotes, and drop empties. A phrase with no ` OR `
/// passes straight through as a one-element list.
pub fn split_legacy_phrase(phrase: &str) -> Vec<String> {
    // Case-insensitive split on ` OR ` (spaces on both sides). We scan the
    // lowercased haystack for the delimiter but slice the original so case is
    // preserved in the resulting terms.
    let lower = phrase.to_lowercase();
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0usize;
    let delim = " or ";
    let mut search_from = 0usize;
    while let Some(rel) = lower[search_from..].find(delim) {
        let idx = search_from + rel;
        parts.push(phrase[start..idx].to_string());
        start = idx + delim.len();
        search_from = start;
    }
    parts.push(phrase[start..].to_string());

    parts
        .into_iter()
        .map(|p| strip_wrapping_quotes(p.trim()).to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Strip a single pair of wrapping double-quotes (`"x"` -> `x`); leave anything
/// else untouched.
fn strip_wrapping_quotes(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Empty/whitespace prompts mean "AI filter disabled"; store them as NULL.
fn normalize_prompt(p: Option<String>) -> Option<String> {
    p.filter(|s| !s.trim().is_empty())
}

pub struct SqliteMonitorRepo {
    pool: SqlitePool,
}

impl SqliteMonitorRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MonitorRepo for SqliteMonitorRepo {
    async fn list(&self, workspace_id: &str) -> Result<Vec<Monitor>, AppError> {
        let rows = sqlx::query_as::<_, MonitorRow>(
            "SELECT id, workspace_id, terms, active, channels, exact_match, case_sensitive, exclude_terms, channel_settings, ai_filter_prompt, created_at, updated_at FROM monitors WHERE workspace_id = ? ORDER BY created_at ASC",
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Monitor::from).collect())
    }

    async fn get(&self, id: &str) -> Result<Option<Monitor>, AppError> {
        let row = sqlx::query_as::<_, MonitorRow>(
            "SELECT id, workspace_id, terms, active, channels, exact_match, case_sensitive, exclude_terms, channel_settings, ai_filter_prompt, created_at, updated_at FROM monitors WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Monitor::from))
    }

    async fn list_active_all(&self) -> Result<Vec<Monitor>, AppError> {
        let rows = sqlx::query_as::<_, MonitorRow>(
            "SELECT id, workspace_id, terms, active, channels, exact_match, case_sensitive, exclude_terms, channel_settings, ai_filter_prompt, created_at, updated_at FROM monitors WHERE active = 1",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Monitor::from).collect())
    }

    async fn create(&self, req: CreateMonitor) -> Result<Monitor, AppError> {
        let id = Ulid::new().to_string();
        let now = chrono::Utc::now().timestamp();
        let terms_json = serde_json::to_string(&req.terms).unwrap_or_else(|_| "[]".to_string());
        let channels_json = serde_json::to_string(&req.channels.unwrap_or_default())
            .unwrap_or_else(|_| "[]".to_string());
        let exclude_terms_json = serde_json::to_string(&req.exclude_terms.unwrap_or_default())
            .unwrap_or_else(|_| "[]".to_string());
        let exact_match = req.exact_match.unwrap_or(false) as i64;
        let case_sensitive = req.case_sensitive.unwrap_or(false) as i64;
        let channel_settings_json = serde_json::to_string(
            &req.channel_settings
                .unwrap_or(serde_json::Value::Object(Default::default())),
        )
        .unwrap_or_else(|_| "{}".to_string());
        let ai_filter_prompt = normalize_prompt(req.ai_filter_prompt);

        sqlx::query(
            "INSERT INTO monitors (id, workspace_id, terms, active, channels, exact_match, case_sensitive, exclude_terms, channel_settings, ai_filter_prompt, created_at, updated_at) VALUES (?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&req.workspace_id)
        .bind(&terms_json)
        .bind(&channels_json)
        .bind(exact_match)
        .bind(case_sensitive)
        .bind(&exclude_terms_json)
        .bind(&channel_settings_json)
        .bind(&ai_filter_prompt)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        self.get(&id).await?.ok_or(AppError::NotFound)
    }

    async fn update(&self, id: &str, req: UpdateMonitor) -> Result<Monitor, AppError> {
        let existing = self.get(id).await?.ok_or(AppError::NotFound)?;
        let now = chrono::Utc::now().timestamp();

        let terms = req.terms.unwrap_or(existing.terms);
        let active = req.active.unwrap_or(existing.active) as i64;
        let channels = req.channels.unwrap_or(existing.channels);
        let exact_match = req.exact_match.unwrap_or(existing.exact_match) as i64;
        let case_sensitive = req.case_sensitive.unwrap_or(existing.case_sensitive) as i64;
        let exclude_terms = req.exclude_terms.unwrap_or(existing.exclude_terms);
        let channel_settings = req.channel_settings.unwrap_or(existing.channel_settings);
        // An explicit empty string clears the prompt (normalized to NULL).
        let ai_filter_prompt = if req.ai_filter_prompt.is_some() {
            normalize_prompt(req.ai_filter_prompt)
        } else {
            existing.ai_filter_prompt
        };

        let terms_json = serde_json::to_string(&terms).unwrap_or_else(|_| "[]".to_string());
        let channels_json = serde_json::to_string(&channels).unwrap_or_else(|_| "[]".to_string());
        let exclude_terms_json =
            serde_json::to_string(&exclude_terms).unwrap_or_else(|_| "[]".to_string());
        let channel_settings_json =
            serde_json::to_string(&channel_settings).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "UPDATE monitors SET terms = ?, active = ?, channels = ?, exact_match = ?, case_sensitive = ?, exclude_terms = ?, channel_settings = ?, ai_filter_prompt = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&terms_json)
        .bind(active)
        .bind(&channels_json)
        .bind(exact_match)
        .bind(case_sensitive)
        .bind(&exclude_terms_json)
        .bind(&channel_settings_json)
        .bind(&ai_filter_prompt)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        self.get(id).await?.ok_or(AppError::NotFound)
    }

    async fn delete(&self, id: &str) -> Result<(), AppError> {
        let affected = sqlx::query("DELETE FROM monitors WHERE id = ?")
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

/// One-time startup repair of legacy OR-phrases (run right after migrations).
///
/// A since-squashed migration backfilled `terms = [phrase]`, a faithful single
/// element. But monitors whose old `phrase` was a malformed `"a" OR "b"` string
/// never actually matched anything under the old fuzzy substring matcher. Here we
/// find every monitor whose `terms` is still a single element containing a
/// top-level ` OR ` and split it into proper bare terms via
/// [`split_legacy_phrase`], persisting the result. Idempotent: a single non-OR
/// term is left untouched, so re-running this does nothing (and it is a no-op on
/// every fresh database, since `terms` starts at `[]`).
pub async fn repair_legacy_or_phrases(pool: &SqlitePool) -> Result<u64, AppError> {
    let rows = sqlx::query_as::<_, (String, String)>("SELECT id, terms FROM monitors")
        .fetch_all(pool)
        .await?;

    let mut repaired = 0u64;
    for (id, terms_json) in rows {
        let terms: Vec<String> = serde_json::from_str(&terms_json).unwrap_or_default();
        // Only single-element lists that still carry a top-level ` OR ` are
        // legacy artifacts; multi-term lists are already in the new model.
        if terms.len() != 1 {
            continue;
        }
        let split = split_legacy_phrase(&terms[0]);
        if split.len() <= 1 && split.first() == terms.first() {
            continue; // no change
        }
        let new_json = serde_json::to_string(&split).unwrap_or_else(|_| "[]".to_string());
        sqlx::query("UPDATE monitors SET terms = ? WHERE id = ?")
            .bind(&new_json)
            .bind(&id)
            .execute(pool)
            .await?;
        tracing::info!(
            "repaired legacy OR-phrase monitor {}: {:?} -> {:?}",
            id,
            terms,
            split
        );
        repaired += 1;
    }
    Ok(repaired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_legacy_phrase_no_or_passes_through() {
        assert_eq!(split_legacy_phrase("rust lang"), vec!["rust lang"]);
        // A bare quoted phrase has its wrapping quotes stripped.
        assert_eq!(split_legacy_phrase("\"nimbusdb\""), vec!["nimbusdb"]);
        // Inner quotes / single quote are left alone.
        assert_eq!(split_legacy_phrase("a\"b"), vec!["a\"b"]);
    }

    #[test]
    fn split_legacy_phrase_splits_quoted_or_list() {
        assert_eq!(
            split_legacy_phrase("\"nimbusdb\" OR \"self-hosted analytics\""),
            vec!["nimbusdb", "self-hosted analytics"]
        );
        assert_eq!(
            split_legacy_phrase("FlaUI OR Ranorex OR \"WinAppDriver\""),
            vec!["FlaUI", "Ranorex", "WinAppDriver"]
        );
    }

    #[test]
    fn split_legacy_phrase_is_case_insensitive_on_delimiter() {
        // ` or ` / ` Or ` / ` oR ` all split; case is preserved in the terms.
        assert_eq!(
            split_legacy_phrase("Foo or Bar Or Baz"),
            vec!["Foo", "Bar", "Baz"]
        );
        // "OR" embedded in a word (not space-delimited) does NOT split.
        assert_eq!(split_legacy_phrase("CORS errors"), vec!["CORS errors"]);
        assert_eq!(
            split_legacy_phrase("normalizer pattern"),
            vec!["normalizer pattern"]
        );
    }

    #[test]
    fn split_legacy_phrase_drops_empties_and_trims() {
        assert_eq!(split_legacy_phrase("  a  OR   OR b  "), vec!["a", "b"]);
        assert_eq!(split_legacy_phrase(""), Vec::<String>::new());
        assert_eq!(split_legacy_phrase("   "), Vec::<String>::new());
    }
}
