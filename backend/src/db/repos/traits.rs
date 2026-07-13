use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::AppError;

// -- Domain models ----------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Monitor {
    pub id: String,
    pub workspace_id: String,
    /// Match-any keyword/phrase list: a post matches this monitor if it contains
    /// ANY of these terms (per `exact_match` / `case_sensitive`) and none of
    /// `exclude_terms`. Each term is a bare literal phrase — never OR/quotes.
    pub terms: Vec<String>,
    pub active: bool,
    pub channels: Vec<String>,
    pub exact_match: bool,
    pub case_sensitive: bool,
    pub exclude_terms: Vec<String>,
    /// Per-channel collection overrides, keyed by channel name (e.g.
    /// `{"reddit": {"subreddits": ["accessibility"]}}`). Shallow-merged over
    /// the channel's global credentials at collection time; monitor keys win.
    pub channel_settings: serde_json::Value,
    /// When set, this monitor's new mentions are held out of the feed until
    /// the AI judge accepts them against this prompt.
    pub ai_filter_prompt: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Mention {
    pub id: String,
    pub monitor_id: String,
    pub channel: String,
    pub external_id: String,
    pub content_text: String,
    pub content_url: String,
    pub author_name: Option<String>,
    pub author_url: Option<String>,
    pub published_at: Option<i64>,
    pub ingested_at: i64,
    pub platform_meta: serde_json::Value,
    /// When the user marked this mention read; `None` = unread.
    pub read_at: Option<i64>,
    /// AI filter verdict: `None` = no AI filtering applied, `pending` =
    /// awaiting judgment (hidden from the feed), `accepted` / `rejected`.
    pub ai_verdict: Option<String>,
    /// The judge's one-sentence explanation for the verdict.
    pub ai_reason: Option<String>,
}

/// A per-workspace notification: a pure delivery endpoint. Every mention that
/// enters the feed fans out to ALL notifications in its workspace — there is no
/// matching, criteria, per-monitor toggle, or frequency. `kind` selects the
/// transport and `config` is the kind-specific JSON:
/// - `webpush`: `{ endpoint, p256dh, auth }` (the browser's subscription)
/// - `webhook`: `{ url }`
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Notification {
    pub id: String,
    pub workspace_id: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub label: Option<String>,
    pub created_at: i64,
}

/// Create request for a notification.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateNotification {
    pub workspace_id: String,
    /// `webpush` | `webhook`.
    pub kind: String,
    /// Kind-specific config JSON (webpush: `{endpoint,p256dh,auth}`; webhook:
    /// `{url}`).
    pub config: serde_json::Value,
    pub label: Option<String>,
}

/// A durable collection target: one consolidated upstream request (an OR-batched
/// global search, or a per-sub search) that the targeted
/// runner pages, banks progress for, and tracks sticky failure status on.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CollectorTarget {
    /// Stable id derived from `channel + kind + descriptor`.
    pub id: String,
    pub channel: String,
    /// `feed` | `search`.
    pub kind: String,
    /// Sub-list or query — human-readable, and the basis for `id`.
    pub descriptor: String,
    /// Timestamp below which this target has been contiguously ingested. The
    /// channel is "caught up" only when all its targets' watermarks are recent.
    pub confirmed_watermark: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub consecutive_failures: i64,
    /// Sticky: a different target succeeding does NOT clear this. Display/health
    /// only — pacing is governed globally by the adaptive rate limiter, not a
    /// per-target backoff.
    pub last_error: Option<String>,
    pub updated_at: i64,
}

/// A durable backfill job: a half-open `[range_start, range_end)` window of one
/// target that still needs older pages walked. Survives restarts; resumes from
/// `next_cursor`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BackfillJob {
    pub id: String,
    pub target_id: String,
    /// Older bound (inclusive).
    pub range_start: i64,
    /// Newer bound — shrinks toward `range_start` as older pages are banked.
    pub range_end: i64,
    /// The `after` token to resume paging from.
    pub next_cursor: Option<String>,
    /// `open` | `done` | `abandoned`.
    pub state: String,
    pub pages_done: i64,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChannelConfig {
    pub channel: String,
    pub enabled: bool,
    pub credentials: serde_json::Value,
    pub poll_interval: i64,
    pub last_polled_at: Option<i64>,
    pub error_message: Option<String>,
    pub updated_at: i64,
    /// The caught-up watermark: how far this channel is contiguously
    /// ingested. Distinct from `last_polled_at`, which is merely the last
    /// attempt time.
    pub caught_up_at: Option<i64>,
    pub max_backfill_days: i64,
}

// -- Request types ----------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateMonitor {
    pub workspace_id: String,
    /// Match-any term list (each a bare literal phrase). A post matches if it
    /// contains ANY term and none of `exclude_terms`.
    pub terms: Vec<String>,
    pub channels: Option<Vec<String>>,
    pub exact_match: Option<bool>,
    pub case_sensitive: Option<bool>,
    pub exclude_terms: Option<Vec<String>>,
    /// Per-channel collection overrides keyed by channel name.
    pub channel_settings: Option<serde_json::Value>,
    /// AI relevance prompt; empty/whitespace is normalized to "disabled".
    pub ai_filter_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateMonitor {
    /// Replace the match-any term list. Omit to leave it unchanged.
    pub terms: Option<Vec<String>>,
    pub channels: Option<Vec<String>>,
    pub exact_match: Option<bool>,
    pub case_sensitive: Option<bool>,
    pub exclude_terms: Option<Vec<String>>,
    pub active: Option<bool>,
    /// Per-channel collection overrides; pass `{}` to clear.
    pub channel_settings: Option<serde_json::Value>,
    /// AI relevance prompt; pass an empty string to clear.
    pub ai_filter_prompt: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewMention {
    pub monitor_id: String,
    pub channel: String,
    pub external_id: String,
    pub content_text: String,
    pub content_url: String,
    pub author_name: Option<String>,
    pub author_url: Option<String>,
    pub published_at: Option<i64>,
    pub platform_meta: serde_json::Value,
    /// `Some("pending")` when the monitor's AI filter must judge this mention
    /// before it becomes visible; `None` = no AI gating.
    pub ai_verdict: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MentionFilter {
    pub workspace_id: Option<String>,
    pub channel: Option<String>,
    pub limit: Option<i64>,
    /// Upper bound (exclusive) on the effective timestamp
    /// (`COALESCE(published_at, ingested_at)`). Paired with `before_id` (the
    /// tiebreaker) this forms the feed's compound keyset pagination cursor
    /// over `ORDER BY effective_ts DESC, id DESC`, so paging one row at a
    /// time can't skip or duplicate rows that tie on `effective_ts`. `before`
    /// alone (no `before_id`) is a plain upper bound with no tiebreak.
    pub before: Option<i64>,
    /// Tiebreaker for `before`: the id of the last row on the previous page.
    /// See `before`.
    pub before_id: Option<String>,
    /// Lower bound (inclusive) on `published_at` — e.g. "last 7 days".
    pub since: Option<i64>,
    pub monitor_id: Option<String>,
    /// Filter by read state: `Some(true)` = read only, `Some(false)` = unread
    /// only, `None` = both.
    pub read: Option<bool>,
    /// Exact AI verdict filter (`pending` / `accepted` / `rejected`).
    pub ai_verdict: Option<String>,
    /// When true, only feed-visible mentions: `ai_verdict` IS NULL (no AI
    /// filtering) or `accepted`. Overrides nothing — combine with care.
    pub ai_visible_only: bool,
}

/// A feed-visible, not-yet-notified mention paired with the workspace it belongs
/// to (resolved via its monitor). Drives the notifier fan-out.
#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub mention: Mention,
    pub workspace_id: String,
}

/// Size and age of the AI-filter backlog for a workspace — how many mentions
/// are still awaiting a verdict (withheld from the feed) and when the oldest was
/// ingested. The feed shows a "N pending AI filter" banner once the oldest has
/// aged past a staleness threshold.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PendingCount {
    pub count: i64,
    /// `ingested_at` (epoch seconds) of the oldest pending mention, or `null`
    /// when nothing is pending.
    pub oldest_ingested_at: Option<i64>,
}

// -- Traits -----------------------------------------------------------------

#[async_trait]
pub trait WorkspaceRepo: Send + Sync {
    async fn list(&self) -> Result<Vec<Workspace>, AppError>;
    async fn get(&self, id: &str) -> Result<Option<Workspace>, AppError>;
    async fn create(&self, name: &str, description: Option<&str>) -> Result<Workspace, AppError>;
    async fn update(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<Workspace, AppError>;
    async fn delete(&self, id: &str) -> Result<(), AppError>;
}

#[async_trait]
pub trait MonitorRepo: Send + Sync {
    async fn list(&self, workspace_id: &str) -> Result<Vec<Monitor>, AppError>;
    async fn get(&self, id: &str) -> Result<Option<Monitor>, AppError>;
    async fn list_active_all(&self) -> Result<Vec<Monitor>, AppError>;
    async fn create(&self, req: CreateMonitor) -> Result<Monitor, AppError>;
    async fn update(&self, id: &str, req: UpdateMonitor) -> Result<Monitor, AppError>;
    async fn delete(&self, id: &str) -> Result<(), AppError>;
}

#[async_trait]
pub trait MentionRepo: Send + Sync {
    async fn list(&self, filter: MentionFilter) -> Result<(Vec<Mention>, bool), AppError>;
    /// Fetch a single mention by id, or `None` if it doesn't exist.
    async fn get(&self, id: &str) -> Result<Option<Mention>, AppError>;
    /// Dedup check is per-monitor: the same external post is stored once per
    /// matching monitor (see `mentions.UNIQUE(monitor_id, channel, external_id)`),
    /// so a post matching two monitors — even across workspaces — is ingested
    /// for both, each with its own read state, AI verdict, and notified_at.
    async fn exists(
        &self,
        monitor_id: &str,
        channel: &str,
        external_id: &str,
    ) -> Result<bool, AppError>;
    async fn insert(&self, new: NewMention) -> Result<Mention, AppError>;
    /// Mark a mention read (`read = true`, stamps `read_at = now`) or unread
    /// (`read = false`, clears `read_at`). Returns the updated mention.
    async fn set_read(&self, id: &str, read: bool) -> Result<Mention, AppError>;
    /// Oldest-first batch of mentions awaiting an AI verdict.
    async fn list_ai_pending(&self, limit: i64) -> Result<Vec<Mention>, AppError>;
    /// Count + oldest-ingested-at of mentions awaiting an AI verdict, scoped to a
    /// workspace (or all workspaces when `None`). Backs the feed backlog banner.
    async fn count_ai_pending(&self, workspace_id: Option<&str>) -> Result<PendingCount, AppError>;
    /// Record the AI verdict (`accepted` / `rejected`) and optional reason.
    /// Returns the updated mention (for SSE broadcast).
    async fn set_ai_verdict(
        &self,
        id: &str,
        verdict: &str,
        reason: Option<&str>,
    ) -> Result<Mention, AppError>;
    /// Increment the failed-judgment counter; returns the new count.
    async fn bump_ai_attempts(&self, id: &str) -> Result<i64, AppError>;
    async fn list_for_channel(&self, channel: &str) -> Result<Vec<Mention>, AppError>;
    async fn delete_many(&self, ids: &[String]) -> Result<u64, AppError>;
    /// Feed-visible mentions that have not yet been notified (`notified_at IS
    /// NULL`), each paired with its workspace id (joined via monitor). The
    /// notifier fan-out source. `limit` bounds one pass; the next pass resumes.
    async fn list_unnotified(&self, limit: i64) -> Result<Vec<PendingNotification>, AppError>;
    /// Stamp `notified_at = now` on the given mentions (fire-once gate).
    async fn mark_notified(&self, ids: &[String]) -> Result<u64, AppError>;
}

#[async_trait]
pub trait NotificationRepo: Send + Sync {
    async fn list_by_workspace(&self, workspace_id: &str) -> Result<Vec<Notification>, AppError>;
    async fn create(
        &self,
        workspace_id: &str,
        kind: &str,
        config: &serde_json::Value,
        label: Option<&str>,
    ) -> Result<Notification, AppError>;
    async fn get(&self, id: &str) -> Result<Option<Notification>, AppError>;
    async fn delete(&self, id: &str) -> Result<(), AppError>;
    /// Remove any webpush notification whose `config.endpoint` matches (for
    /// pruning a subscription the push service reports as Gone). Returns how
    /// many rows were removed.
    async fn delete_by_endpoint(&self, endpoint: &str) -> Result<u64, AppError>;
}

#[async_trait]
pub trait ChannelRepo: Send + Sync {
    async fn list(&self) -> Result<Vec<ChannelConfig>, AppError>;
    async fn get(&self, channel: &str) -> Result<Option<ChannelConfig>, AppError>;
    /// Create or update a channel config. `credentials: None` leaves the
    /// stored credentials untouched (preserved across updates that only
    /// toggle `enabled`/`poll_interval`); a brand-new channel row defaults to
    /// `{}` when credentials are omitted.
    async fn upsert(
        &self,
        channel: &str,
        enabled: bool,
        credentials: Option<serde_json::Value>,
        poll_interval: i64,
    ) -> Result<ChannelConfig, AppError>;
    async fn update_polled(
        &self,
        channel: &str,
        error_message: Option<&str>,
    ) -> Result<(), AppError>;
    /// Stamp `caught_up_at = now` (a full pass completed without errors).
    async fn set_caught_up_now(&self, channel: &str) -> Result<(), AppError>;
    /// Like `set_caught_up_now` but stamps an explicit value (the channel's
    /// minimum confirmed watermark), so `caught_up_at` only advances as far as
    /// the *least* caught-up target. `None` leaves it untouched.
    async fn set_caught_up_at(&self, channel: &str, value: Option<i64>) -> Result<(), AppError>;
}

#[async_trait]
pub trait CollectorTargetRepo: Send + Sync {
    /// Create (or fetch the existing) target for `channel/kind/descriptor`. The
    /// id is derived deterministically so re-planning the same target is
    /// idempotent. Existing status (watermark, failures) is preserved.
    async fn upsert_target(
        &self,
        channel: &str,
        kind: &str,
        descriptor: &str,
    ) -> Result<CollectorTarget, AppError>;
    async fn get_target(&self, id: &str) -> Result<Option<CollectorTarget>, AppError>;
    /// Live (non-retired) targets for `channel`.
    async fn list_targets(&self, channel: &str) -> Result<Vec<CollectorTarget>, AppError>;

    // -- Lifecycle (reconcile + membership) ---------------------------------
    /// Replace the monitor membership of a target with `monitor_ids` (the
    /// monitors the current plan says this target serves). Drives cascade on
    /// monitor/workspace delete and lets reconcile know what's still referenced.
    async fn set_target_members(
        &self,
        channel: &str,
        target_id: &str,
        monitor_ids: &[String],
    ) -> Result<(), AppError>;
    /// Soft-retire every live target of `channel` whose id is NOT in `live_ids`
    /// (the current plan). Returns how many were retired. An empty `live_ids`
    /// retires all live targets (the channel has no targets now). Preserves the
    /// row (watermark/cursor) so a re-added target resumes via `upsert_target`.
    async fn reconcile_targets(&self, channel: &str, live_ids: &[String]) -> Result<u64, AppError>;
    /// Hard-delete targets retired before `older_than` (cascades their jobs +
    /// memberships). Returns how many were purged.
    async fn purge_retired(&self, older_than: i64) -> Result<u64, AppError>;
    /// Record a successful walk: clears failures/last_error, stamps
    /// `last_success_at`/`last_attempt_at`, and (when `confirmed_watermark` is
    /// `Some`) advances the watermark.
    async fn record_target_success(
        &self,
        id: &str,
        confirmed_watermark: Option<i64>,
    ) -> Result<(), AppError>;
    /// Record a failure: increments `consecutive_failures`, stamps
    /// `last_attempt_at`/`last_error`. STICKY — a later success on a *different*
    /// target does not clear this one. Recorded for display/health only; it does
    /// not throttle (the adaptive rate limiter does).
    async fn record_target_failure(&self, id: &str, error: &str) -> Result<(), AppError>;

    // -- Backfill jobs ------------------------------------------------------
    /// Enqueue an open job for `[range_start, range_end)`. Returns the existing
    /// open job covering the same window if one already exists (idempotent).
    async fn enqueue_job(
        &self,
        target_id: &str,
        range_start: i64,
        range_end: i64,
    ) -> Result<BackfillJob, AppError>;
    async fn get_job(&self, id: &str) -> Result<Option<BackfillJob>, AppError>;
    /// Open jobs for every target of `channel`, oldest-created first.
    async fn list_open_jobs_for_channel(&self, channel: &str)
        -> Result<Vec<BackfillJob>, AppError>;
    async fn list_open_jobs_for_target(
        &self,
        target_id: &str,
    ) -> Result<Vec<BackfillJob>, AppError>;
    /// Bank progress on a job: store the resume cursor, bump `pages_done`, and
    /// shrink `range_end` to the oldest point reached so far.
    async fn update_job_progress(
        &self,
        id: &str,
        next_cursor: Option<&str>,
        pages_done: i64,
        range_end: i64,
    ) -> Result<(), AppError>;
    async fn complete_job(&self, id: &str) -> Result<(), AppError>;
    /// Set a terminal/other state (`done` | `abandoned`) and record why.
    async fn mark_job(&self, id: &str, state: &str, error: Option<&str>) -> Result<(), AppError>;
}
