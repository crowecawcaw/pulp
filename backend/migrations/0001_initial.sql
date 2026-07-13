-- Initial schema (squashed from the pre-release migration history: this is a
-- pre-release project with no users, so the 19 incremental migrations that
-- built and reshaped this schema over time were collapsed into a single
-- migration that recreates the exact final schema and seed data directly,
-- rather than replaying now-irrelevant intermediate states (columns added
-- then dropped, tables created then dropped, data fixups for rows that no
-- longer exist in a fresh database). Equivalence with the old multi-migration
-- history was verified by diffing a full schema+data dump of both paths.

CREATE TABLE workspaces (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  description TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

-- A monitor watches for any of `terms` (match-ANY semantics; none of
-- `exclude_terms`) across its `channels`. `channel_settings` is a JSON object
-- keyed by channel name that is shallow-merged OVER the channel's global
-- `channel_configs.credentials` at collection time (monitor keys win), so
-- per-monitor scoping (e.g. subreddits, only_repos) normally lives here. When
-- `ai_filter_prompt` is set, newly collected mentions are held out of the feed
-- pending AI judgment (see mentions.ai_verdict below).
CREATE TABLE monitors (
  id                TEXT PRIMARY KEY,
  workspace_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  active            INTEGER NOT NULL DEFAULT 1,
  channels          TEXT NOT NULL DEFAULT '[]',
  exact_match       INTEGER NOT NULL DEFAULT 0,
  case_sensitive    INTEGER NOT NULL DEFAULT 0,
  exclude_terms     TEXT NOT NULL DEFAULT '[]',
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL,
  channel_settings  TEXT NOT NULL DEFAULT '{}',
  ai_filter_prompt  TEXT,
  terms             TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX idx_monitors_workspace ON monitors(workspace_id);

-- A collected mention. `read_at` tracks feed read/unread state. `ai_verdict`
-- is NULL when no AI filtering applies, 'pending' while awaiting judgment
-- (hidden from the feed), or 'accepted'/'rejected' once judged (rejected
-- mentions are kept but hidden from the default feed view — soft filter);
-- `ai_attempts` caps retries before failing open to 'accepted'. `notified_at`
-- is the fire-once marker for workspace notification fan-out. Dedup is
-- per-monitor (UNIQUE(monitor_id, channel, external_id)): the same external
-- post is stored once per matching monitor, so a post matching two monitors —
-- even across different workspaces — is ingested as its own row for each,
-- with read state, AI verdict, and notified_at all scoped correctly per
-- monitor/workspace.
CREATE TABLE mentions (
  id               TEXT PRIMARY KEY,
  monitor_id       TEXT NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
  channel          TEXT NOT NULL,
  external_id      TEXT NOT NULL,
  content_text     TEXT NOT NULL,
  content_url      TEXT NOT NULL,
  author_name      TEXT,
  author_url       TEXT,
  published_at     INTEGER,
  ingested_at      INTEGER NOT NULL,
  platform_meta    TEXT NOT NULL DEFAULT '{}',
  read_at          INTEGER,
  ai_verdict       TEXT,
  ai_reason        TEXT,
  ai_attempts      INTEGER NOT NULL DEFAULT 0,
  notified_at      INTEGER,
  UNIQUE(monitor_id, channel, external_id)
);

CREATE INDEX idx_mentions_monitor    ON mentions(monitor_id, published_at DESC);
CREATE INDEX idx_mentions_channel    ON mentions(channel, published_at DESC);
CREATE INDEX idx_mentions_read       ON mentions(read_at);
CREATE INDEX idx_mentions_ai_verdict ON mentions(ai_verdict);
-- Serves the feed's hot ordering (`ORDER BY COALESCE(published_at,
-- ingested_at) DESC, id DESC` — see mention.rs `list()`).
CREATE INDEX idx_mentions_feed_order ON mentions(COALESCE(published_at, ingested_at) DESC, id DESC);
-- Serves the notifier's 30s scan for feed-visible, not-yet-notified mentions
-- (`WHERE notified_at IS NULL ... ORDER BY ingested_at ASC` — see
-- mention.rs `list_unnotified()`).
CREATE INDEX idx_mentions_unnotified ON mentions(ingested_at) WHERE notified_at IS NULL;

-- One row per known channel (see collectors::CHANNELS). Credentials/scoping
-- live in `credentials` JSON; per-monitor `channel_settings` overrides it at
-- collection time. `max_backfill_days` bounds how far back backfill jobs
-- reach on a first poll. `caught_up_at` is the caught-up watermark: how far
-- the channel is contiguously ingested (distinct from `last_polled_at`, which
-- is merely the last attempt time).
CREATE TABLE channel_configs (
  channel            TEXT PRIMARY KEY,
  enabled            INTEGER NOT NULL DEFAULT 0,
  credentials        TEXT NOT NULL DEFAULT '{}',
  poll_interval      INTEGER NOT NULL DEFAULT 900,
  last_polled_at     INTEGER,
  error_message      TEXT,
  updated_at         INTEGER NOT NULL,
  caught_up_at       INTEGER,
  max_backfill_days  INTEGER NOT NULL DEFAULT 7
);

-- Seed default channel rows so they always exist. Only channels with a real
-- collector (see collectors::CHANNELS) are seeded — a row here with no
-- collector would show up in GET /api/channels but could never be enabled.
INSERT OR IGNORE INTO channel_configs(channel, enabled, updated_at) VALUES
  ('hackernews',    0, unixepoch()),
  ('reddit',        0, unixepoch()),
  ('github',        0, unixepoch());

-- Durable collection targets and backfill jobs for the auto-recovering
-- collector pipeline. A "target" is one consolidated upstream request (an
-- OR-batched global search, or a per-sub search): the unit at which
-- watermarks, sticky failure status, and backfill progress are tracked.
-- `retired_at` soft-retires a target no longer in the collection
-- plan (NULL = live) instead of deleting it, preserving its watermark/cursor
-- so a re-added monitor resumes rather than re-backfilling. Throttling is
-- governed solely by the adaptive AIMD rate limiter (per-IP, global to the
-- channel lane) — there is no per-target backoff column.
CREATE TABLE collector_targets (
  id                    TEXT PRIMARY KEY,           -- stable hash of channel+kind+descriptor
  channel               TEXT NOT NULL,
  kind                  TEXT NOT NULL,               -- 'feed' | 'search'
  descriptor            TEXT NOT NULL,               -- sub-list or query (for display + id)
  confirmed_watermark   INTEGER,                     -- ts below which contiguously ingested
  last_success_at       INTEGER,
  last_attempt_at       INTEGER,
  consecutive_failures  INTEGER NOT NULL DEFAULT 0,
  last_error            TEXT,                        -- sticky: not cleared by a different target's success
  updated_at            INTEGER NOT NULL,
  retired_at            INTEGER                      -- NULL = live
);

CREATE INDEX idx_collector_targets_channel      ON collector_targets(channel);
CREATE INDEX idx_collector_targets_channel_live ON collector_targets(channel, retired_at);

-- Jobs are durable so a crash/throttle resumes from the banked cursor instead
-- of re-losing work.
CREATE TABLE collector_backfill_jobs (
  id           TEXT PRIMARY KEY,
  target_id    TEXT NOT NULL REFERENCES collector_targets(id) ON DELETE CASCADE,
  range_start  INTEGER NOT NULL,                    -- older bound (inclusive)
  range_end    INTEGER NOT NULL,                    -- newer bound (exclusive-ish)
  next_cursor  TEXT,                                -- the `after` token to resume paging
  state        TEXT NOT NULL DEFAULT 'open',        -- 'open' | 'done' | 'abandoned'
  pages_done   INTEGER NOT NULL DEFAULT 0,
  attempts     INTEGER NOT NULL DEFAULT 0,
  last_error   TEXT,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);

CREATE INDEX idx_collector_backfill_jobs_target_state ON collector_backfill_jobs(target_id, state);

-- Which monitors each collector target serves. Rebuilt from the collection
-- plan every pass; ON DELETE CASCADE drops a target's memberships when its
-- monitor (or workspace) is deleted, so a deleted monitor's target falls out
-- of the plan and is retired on the next pass.
CREATE TABLE target_monitors (
  target_id  TEXT NOT NULL REFERENCES collector_targets(id) ON DELETE CASCADE,
  monitor_id TEXT NOT NULL REFERENCES monitors(id)          ON DELETE CASCADE,
  channel    TEXT NOT NULL,
  PRIMARY KEY (target_id, monitor_id)
);

CREATE INDEX idx_target_monitors_monitor ON target_monitors(monitor_id);

-- Per-workspace notifications: a notification is a pure delivery endpoint
-- scoped to a workspace; every feed-visible mention fans out to ALL
-- notifications in its workspace (no matching, no criteria, no per-monitor
-- toggle, no frequency/digest) — fire-once is gated by mentions.notified_at.
-- Kinds: 'webpush' (config JSON: {endpoint, p256dh, auth}) and 'webhook'
-- (config JSON: {url}).
CREATE TABLE notifications (
  id           TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  kind         TEXT NOT NULL,          -- 'webpush' | 'webhook'
  config       TEXT NOT NULL,          -- JSON; webpush: {endpoint,p256dh,auth}, webhook: {url}
  label        TEXT,
  created_at   INTEGER NOT NULL
);

CREATE INDEX idx_notifications_workspace ON notifications(workspace_id);
