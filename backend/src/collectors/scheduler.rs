//! Generic, auto-recovering collection runner for "targeted" collectors.
//!
//! A *target* is one consolidated upstream request (an OR-batched global search,
//! or a per-sub search). The runner pages each target,
//! banks progress after every page (so a crash/throttle resumes from the banked
//! cursor), keeps the live feed current via a head fetch, detects gaps between
//! the newest page and the durable `confirmed_watermark`, enqueues durable
//! backfill jobs to fill them, drains those jobs with leftover limiter budget,
//! and tracks sticky per-target failure status.
//!
//! It is deliberately collector-agnostic: Reddit is the first consumer, but any
//! collector that implements [`TargetedCollector`] gets the same machinery. The
//! ordinary [`Collector::fetch_pass`] path (HackerNews, GitHub) is untouched —
//! [`crate::collectors::run_pass`] dispatches to whichever a collector supports.
//!
//! All pacing is driven by the generic [`crate::ratelimit`] throttle: one lane
//! per endpoint class so a throttle on `search` doesn't starve `feed`.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use std::time::Duration;

use crate::collectors::{Collector, MonitorFetch, RawMention};
use crate::db::repos::traits::CollectorTarget;
use crate::ratelimit::{AdaptiveConfig, KeyedThrottle, Outcome};
use crate::state::AppState;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// ───────────────────────────────────────────────────────────────────────────
// Throttle configuration
// ───────────────────────────────────────────────────────────────────────────

/// Build the shared per-channel throttle map. Reddit uses ONE lane for the whole
/// channel (every workspace's feed + search requests), because Reddit's
/// unauthenticated limit is per-IP and global — separate per-endpoint lanes would
/// each get the full budget and together blow past it (the original bug: a
/// `reddit:feed` lane + a `reddit:search` lane each at ~10/min = ~20/min).
///
/// One shared lane means the budget is split across all targets: more monitors
/// ⇒ each target waits a full `interval` for its turn ⇒ polling slows
/// automatically, staying under the cap. Pacing is **pure interval (no burst,
/// no token bucket)**: a single persisted `next_allowed` instant, anchored at
/// *issue* time, spaces every request — including the FIRST of each pass — by the
/// current interval. This kills the original failure mode where a refilling
/// token bucket handed the first target of every pass a free request while the
/// rest 429'd on a penalized IP.
///
/// The interval **self-tunes via AIMD on the interval itself**. It starts at
/// `initial_interval` (`60 / PULP_REDDIT_RATE_PER_MIN`s, default 7.5s for 8/min)
/// and on a 429 it multiplicatively doubles (`grow_factor = 2.0`) — walking the
/// rate *down* fast to what the IP tolerates, respecting any `Retry-After`.
/// Recovery is a *gentle* additive shrink (`shrink_step = 1s` per success). Since
/// tightening is multiplicative and loosening additive, **failure dominates
/// recovery**: a recurring 429 drives the interval up and HOLDS it (it does not
/// re-inflate back to the floor), converging on a sustainable interval shared
/// fairly by all targets.
///
/// The floor `min_interval` (`60 / PULP_REDDIT_MAX_RATE_PER_MIN`s, default 6s for
/// 10/min) is Reddit's documented unauth budget — the *fastest* we ever pace. The
/// ceiling `max_interval` (120s = 0.5/min) is the slowest. There is no circuit
/// breaker and no per-target backoff: a single-IP poller doesn't need either —
/// the AIMD interval backs all targets off on a 429, and a failing target simply
/// retries next poll cycle.
/// True when `REDDIT_API_BASE` is set AND points at loopback — the shape of
/// every test mock server (`tests/common/mock_reddit.rs` binds
/// `127.0.0.1:0`). This used to be gated on `REDDIT_API_BASE` being set at
/// all, which meant a real deployment that legitimately overrides
/// `REDDIT_API_BASE` — to route through a self-hosted mirror or corporate
/// proxy, say — silently got ALL Reddit rate limiting disabled, risking a
/// real ban. Only a same-machine mock should ever get the unthrottled test
/// config; any other host still gets normal AIMD pacing.
fn reddit_base_is_local_test_mock() -> bool {
    let Ok(base) = std::env::var("REDDIT_API_BASE") else {
        return false;
    };
    // Deliberately avoids a URL-parsing dependency for this one check: strip
    // the scheme, then take everything up to the next `/` or `:` as the host.
    let after_scheme = base.split("://").nth(1).unwrap_or(base.as_str());
    let host = after_scheme.split(['/', ':']).next().unwrap_or("");
    matches!(host, "127.0.0.1" | "localhost" | "::1") || host.starts_with("127.")
}

pub fn default_throttles() -> KeyedThrottle<String> {
    // Tests point REDDIT_API_BASE at a local mock; there's no real budget to
    // respect, so run effectively unthrottled (tiny interval, no growth) to keep
    // the suite fast.
    if reddit_base_is_local_test_mock() {
        return KeyedThrottle::new(AdaptiveConfig {
            initial_interval: Duration::from_millis(1),
            min_interval: Duration::from_millis(1),
            max_interval: Duration::from_millis(1),
            grow_factor: 1.0, // never tighten in tests
            shrink_step: Duration::ZERO,
            max_retry_after: Duration::from_millis(1),
        });
    }
    let per_min: f64 = std::env::var("PULP_REDDIT_RATE_PER_MIN")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|v: &f64| *v > 0.0)
        .unwrap_or(8.0);
    // Floor interval = fastest pacing: Reddit's documented unauth budget (~10/min
    // → 6s). The initial rate can't exceed this ceiling-rate, so the initial
    // interval can't be tighter than the floor.
    let max_per_min: f64 = std::env::var("PULP_REDDIT_MAX_RATE_PER_MIN")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|v: &f64| *v > 0.0)
        .unwrap_or(10.0)
        .max(per_min);
    let initial_interval = Duration::from_secs_f64(60.0 / per_min); // 8/min → 7.5s
    let min_interval = Duration::from_secs_f64(60.0 / max_per_min); // 10/min → 6s
    KeyedThrottle::new(AdaptiveConfig {
        initial_interval,
        // Floor interval — never pace faster than Reddit's documented budget.
        min_interval,
        // Ceiling interval — slowest pacing (0.5 req/min = 1 request per 2 min). A
        // penalized IP can tolerate well under the documented ~10/min (observed
        // ~0.8/min while throttled), so the controller must be able to crawl this
        // slow to find a sustainable interval.
        max_interval: Duration::from_secs(120),
        // Double the interval on a 429 (multiplicative tighten / auto-backoff).
        grow_factor: 2.0,
        // Gentle additive loosen — one step of ~1s of interval per success, so a
        // run of successes nibbles the interval back down but a recurring 429
        // (×2) outpaces it and holds the interval elevated.
        shrink_step: Duration::from_secs(1),
        max_retry_after: Duration::from_secs(300),
    })
}

// ───────────────────────────────────────────────────────────────────────────
// Targeted collector trait + shared types
// ───────────────────────────────────────────────────────────────────────────

/// A parsed upstream entry, generic over the source. The runner only needs an
/// id (for dedup + cursor), an optional timestamp (for watermark/gap logic),
/// and the per-monitor mention mapping (which the collector owns).
pub struct ParsedItem {
    /// Stable external id; doubles as the dedup key and pagination cursor.
    pub external_id: String,
    /// Publish time, if the source carries one.
    pub published_at: Option<i64>,
}

/// The kind of a target, persisted for display. Reddit uses only `Search`
/// (global search); the firehose `Feed` kind was removed. Kept as an enum so
/// future targeted collectors can add their own kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Search,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Search => "search",
        }
    }
}

/// One consolidated request the runner will page, bank, and track. The collector
/// builds these from the active monitors; the runner owns persistence + pacing.
pub struct PlannedTarget {
    pub kind: TargetKind,
    /// Human-readable + identity basis (sub-list or query).
    pub descriptor: String,
    /// Throttle lane key — usually `"{channel}:{kind}"`.
    pub lane: String,
    /// Member monitor ids this target's entries are matched against.
    pub member_monitor_ids: Vec<String>,
    /// Opaque, collector-specific request parameters (e.g. the base URL/query).
    /// The runner passes this back to [`TargetedCollector::fetch_target_page`].
    pub request: TargetRequest,
}

/// Collector-specific request descriptor. Kept as a small struct rather than a
/// trait object so it's `Send + Clone` and trivially testable.
#[derive(Debug, Clone)]
pub struct TargetRequest {
    /// Fully-formed base URL (without the `&after=` cursor).
    pub url: String,
    /// User-Agent to send.
    pub user_agent: String,
}

/// The result of fetching ONE page of ONE target.
pub struct TargetPage {
    /// Parsed items, newest-first as the source returns them.
    pub items: Vec<ParsedItem>,
    /// The cursor to fetch the next (older) page, or `None` at the end.
    pub next_cursor: Option<String>,
    /// How the HTTP call went, for the throttle to adapt on.
    pub outcome: Outcome,
    /// Human-readable detail for a non-success `outcome` (e.g. `"request timed
    /// out"`, `"HTTP 503"`, `"body read error: …"`), recorded into the target's
    /// sticky `last_error`. `None` on success. Without this the runner could only
    /// record a generic `"fetch failed"`, hiding timeout-vs-reset-vs-5xx.
    pub error: Option<String>,
    /// Per-monitor mentions parsed from `items` (monitor id → its matches).
    pub mentions: Vec<(String, Vec<RawMention>)>,
}

/// A collector that opts into durable, target-based, auto-recovering collection.
/// Default-not-implemented: collectors that don't implement it keep the legacy
/// [`Collector::fetch_pass`] path.
#[async_trait]
pub trait TargetedCollector: Collector {
    /// Build the durable targets for the current active monitors.
    fn plan_targets(&self, inputs: &[MonitorFetch<'_>]) -> Vec<PlannedTarget>;

    /// Fetch ONE page of one target (page 1 when `cursor` is `None`). Returns the
    /// parsed items, the next cursor, the raw [`Outcome`] for the limiter, and
    /// the per-monitor mentions. Implementations must NOT sleep/pace — the runner
    /// owns pacing via the throttle.
    async fn fetch_target_page(
        &self,
        target: &PlannedTarget,
        http: &reqwest::Client,
        cursor: Option<&str>,
    ) -> TargetPage;
}

// ───────────────────────────────────────────────────────────────────────────
// Pure logic (unit-tested in isolation)
// ───────────────────────────────────────────────────────────────────────────

/// Map an HTTP status (or transport failure) to a throttle [`Outcome`].
/// `retry_after` is the parsed `Retry-After` header value (seconds), if present.
pub fn outcome_for_status(status: Option<u16>, retry_after: Option<Duration>) -> Outcome {
    match status {
        Some(429) => Outcome::Throttled { retry_after },
        Some(s) if (200..300).contains(&s) => Outcome::Success,
        // 5xx and everything else (incl. transport error: None) → Failure.
        _ => Outcome::Failure,
    }
}

/// Parse a `Retry-After` header value in its seconds form (the only form Reddit
/// emits). HTTP-date form is treated as absent (`None`) — the limiter then falls
/// back to its own backoff.
pub fn parse_retry_after(header: Option<&str>) -> Option<Duration> {
    header
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// The oldest timestamp among a page's items (the deepest point reached), or
/// `None` if none carried a timestamp.
pub fn oldest_ts(items: &[ParsedItem]) -> Option<i64> {
    items.iter().filter_map(|i| i.published_at).min()
}

/// The newest timestamp among a page's items.
pub fn newest_ts(items: &[ParsedItem]) -> Option<i64> {
    items.iter().filter_map(|i| i.published_at).max()
}

/// Gap detection for a head walk (which may have paged past page 1 this cycle).
///
/// Returns `Some((range_start, range_end))` describing a backfill window when
/// the *deepest* item reached this cycle (across every page fetched, not just
/// page 1) is still *newer* than the target's `confirmed_watermark` — i.e.
/// there's a hole between what we just ingested and what we'd previously
/// confirmed contiguous. The window is `[watermark, deepest_oldest)`.
///
/// Callers must pass the oldest timestamp seen across *all* pages fetched this
/// cycle, not just page 1 — using only page 1 would report a gap even when a
/// later page in the same cycle already paged down past the watermark and
/// closed it, enqueueing a spurious backfill job for a hole that isn't there.
///
/// Returns `None` when there's no gap: either we have no prior watermark (first
/// ever run — the head walk itself establishes it), or the walk already reached
/// back to/under the watermark (contiguous, no hole).
pub fn detect_gap(
    deepest_oldest: Option<i64>,
    confirmed_watermark: Option<i64>,
) -> Option<(i64, i64)> {
    let (Some(oldest), Some(watermark)) = (deepest_oldest, confirmed_watermark) else {
        return None;
    };
    if oldest > watermark {
        Some((watermark, oldest))
    } else {
        None
    }
}

/// The new `confirmed_watermark` after a head walk.
///
/// The watermark advances to the oldest contiguously-walked point when the walk
/// reached back to/under the previous watermark (no gap) — then we've proven
/// contiguity down to `page_oldest`.
///
/// When a gap remains above the previous watermark, `reached_end` decides what
/// happens next:
/// - `reached_end == false`: we simply stopped early this cycle (page budget,
///   dedup stop, etc.) and might close the gap by paging further on a later
///   cycle, so the watermark must NOT advance past the hole yet — return the
///   previous value unchanged (a backfill job enqueued by [`detect_gap`] will
///   try to close it).
/// - `reached_end == true`: the walk exhausted the source itself (no further
///   cursor) *without* ever reaching the previous watermark. This happens when
///   items older than `page_oldest` have aged out of the upstream's search
///   horizon (Reddit's search index only covers a rolling window) — the
///   anchor the old watermark depends on is gone and paging further can never
///   prove contiguity down to it, no matter how many more cycles we try. Holding
///   the watermark at `prev` forever would wedge it permanently. Instead, treat
///   `page_oldest` as the new floor: the gap above it is likely unrecoverable,
///   but the backfill job [`detect_gap`] queued for it will confirm that (and
///   abandon itself) rather than the watermark blocking forever.
pub fn advance_watermark(
    page_oldest: Option<i64>,
    confirmed_watermark: Option<i64>,
    reached_end: bool,
) -> Option<i64> {
    match (page_oldest, confirmed_watermark) {
        // First ever run: only commit a watermark once we've walked to the end
        // (otherwise older items beyond the page remain unconfirmed).
        (Some(oldest), None) => {
            if reached_end {
                Some(oldest)
            } else {
                None
            }
        }
        (Some(oldest), Some(prev)) => {
            if oldest <= prev {
                // Contiguous down to `oldest` — the walk overlapped (or reached)
                // the prior watermark, so there's no hole between them.
                Some(oldest)
            } else if reached_end {
                // Source exhausted before reaching `prev`: the older items are
                // unreachable this cycle (and likely gone from the upstream's
                // search horizon). Move past the hole rather than wedging.
                Some(oldest)
            } else {
                // A gap remains above `prev` and more paging might still close
                // it later; don't move the watermark yet.
                Some(prev)
            }
        }
        (None, prev) => prev,
    }
}

/// Whether a backfill job's remaining window has fallen entirely outside the
/// `max_backfill` cutoff (so it should be abandoned rather than chased forever).
/// `range_end` is the job's (shrinking) newer bound; once even the newest part
/// of what's left is older than the cutoff, nothing in range is worth fetching.
pub fn job_out_of_window(range_end: i64, cutoff: i64) -> bool {
    range_end <= cutoff
}

// ───────────────────────────────────────────────────────────────────────────
// Target health — the single classifier shared by the per-target API view and
// the channel-level degraded banner, so the two can never disagree.
// ───────────────────────────────────────────────────────────────────────────

/// Coarse health of one collection target, derived purely from its sticky
/// status fields. This is the *single source of truth* for both the per-target
/// status badge (`GET /api/channels/{channel}/targets`) and the channel-level
/// degraded summary — both call [`target_health`] / [`summarize_targets`], so a
/// banner count can never drift from the table. It is time-independent: with
/// throttling governed globally by the rate limiter (no per-target backoff
/// clock), a target's health depends only on its latest attempt outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TargetHealth {
    /// Succeeding, no outstanding failures.
    Healthy,
    /// Never attempted (no success and no failures recorded yet).
    Idle,
    /// Latest attempt(s) failed with a rate-limit (429) error.
    Throttled,
    /// Latest attempt(s) failed with a non-throttle error (5xx / transport).
    Failing,
}

impl TargetHealth {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetHealth::Healthy => "healthy",
            TargetHealth::Idle => "idle",
            TargetHealth::Throttled => "throttled",
            TargetHealth::Failing => "failing",
        }
    }

    /// Whether this state should count toward the channel's "degraded" banner
    /// (i.e. anything that isn't healthy or idle).
    pub fn is_degraded(self) -> bool {
        matches!(self, TargetHealth::Throttled | TargetHealth::Failing)
    }
}

/// Whether a sticky `last_error` string looks like a rate-limit (429) response.
fn looks_rate_limited(err: Option<&str>) -> bool {
    err.map(|e| {
        let e = e.to_lowercase();
        e.contains("429")
            || e.contains("rate limit")
            || e.contains("rate-limit")
            || e.contains("too many requests")
    })
    .unwrap_or(false)
}

/// Classify one target's health from its latest attempt. Pure; unit-tested below.
pub fn target_health(t: &CollectorTarget) -> TargetHealth {
    if t.consecutive_failures == 0 {
        return if t.last_success_at.is_some() {
            TargetHealth::Healthy
        } else {
            TargetHealth::Idle
        };
    }
    // Failing now (consecutive_failures > 0): split by error kind.
    if looks_rate_limited(t.last_error.as_deref()) {
        TargetHealth::Throttled
    } else {
        TargetHealth::Failing
    }
}

/// Per-health counts plus a ready-to-display degraded message, summarizing a
/// channel's targets at time `now`. The `message` is `None` when nothing is
/// degraded; otherwise it leads with "rate-limited" when any target is throttled
/// (so the banner reads as a throttle), else "degraded".
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct ChannelHealthSummary {
    pub total: usize,
    pub healthy: usize,
    pub idle: usize,
    pub throttled: usize,
    pub failing: usize,
    /// Targets that are throttled or failing (the banner count).
    pub degraded: usize,
    /// A human-readable degraded summary, or `None` when all targets are fine.
    pub message: Option<String>,
}

/// Summarize a channel's targets into per-health counts + a banner message,
/// using the same [`target_health`] classifier the per-target view uses.
pub fn summarize_targets(targets: &[CollectorTarget]) -> ChannelHealthSummary {
    let mut s = ChannelHealthSummary {
        total: targets.len(),
        healthy: 0,
        idle: 0,
        throttled: 0,
        failing: 0,
        degraded: 0,
        message: None,
    };
    for t in targets {
        match target_health(t) {
            TargetHealth::Healthy => s.healthy += 1,
            TargetHealth::Idle => s.idle += 1,
            TargetHealth::Throttled => s.throttled += 1,
            TargetHealth::Failing => s.failing += 1,
        }
    }
    s.degraded = s.throttled + s.failing;
    s.message = if s.degraded == 0 {
        None
    } else if s.throttled > 0 {
        Some(format!(
            "rate-limited: {}/{} targets degraded",
            s.degraded, s.total
        ))
    } else {
        Some(format!(
            "degraded: {}/{} targets degraded",
            s.degraded, s.total
        ))
    };
    s
}

// ───────────────────────────────────────────────────────────────────────────
// The runner
// ───────────────────────────────────────────────────────────────────────────

/// How many backfill pages to walk per target per cycle, so one slow target
/// can't monopolize the limiter budget for the whole channel.
const MAX_BACKFILL_PAGES_PER_CYCLE: usize = 3;
/// Hard cap on head-walk pages per target per cycle (gap detection + a job
/// handle deeper history).
const MAX_HEAD_PAGES_PER_CYCLE: usize = 5;

/// How long a soft-retired target's row is kept before it's GC'd. The grace
/// window lets a briefly-removed-then-readded monitor resume from its preserved
/// watermark instead of re-backfilling from scratch.
const TARGET_RETIRE_GRACE_SECS: i64 = 7 * 86_400;

/// Drive one targeted collection pass. Returns the channel-level error summary
/// (sticky/degraded signal) if any target is unhealthy, plus the minimum
/// confirmed watermark across the channel's targets (the point at which the
/// channel is "caught up").
pub async fn run_targeted_pass(
    state: &Arc<AppState>,
    collector: &dyn TargetedCollector,
    inputs: &[MonitorFetch<'_>],
    since: Option<i64>,
    max_backfill_cutoff: i64,
) -> TargetedPassOutcome {
    let channel = collector.channel();
    let planned = collector.plan_targets(inputs);

    // 1. Upsert every planned target so its durable row + status exists, and
    //    record its monitor membership (drives cascade on monitor delete).
    let mut target_ids: Vec<(String, &PlannedTarget)> = Vec::new();
    for t in &planned {
        match state
            .collector_targets
            .upsert_target(channel, t.kind.as_str(), &t.descriptor)
            .await
        {
            Ok(row) => {
                if let Err(e) = state
                    .collector_targets
                    .set_target_members(channel, &row.id, &t.member_monitor_ids)
                    .await
                {
                    tracing::error!("set target members failed for {}: {:?}", t.descriptor, e);
                }
                target_ids.push((row.id, t));
            }
            Err(e) => tracing::error!("upsert target failed for {}: {:?}", t.descriptor, e),
        }
    }

    // 1b. Reconcile: soft-retire targets no longer in the plan (monitor edited or
    //     deleted), then GC rows retired beyond the grace window. The live set is
    //     derived from the plan directly so a transient upsert error can't make us
    //     retire a still-wanted target.
    let live_ids: Vec<String> = planned
        .iter()
        .map(|t| {
            crate::db::repos::collector_target::target_id(channel, t.kind.as_str(), &t.descriptor)
        })
        .collect();
    match state
        .collector_targets
        .reconcile_targets(channel, &live_ids)
        .await
    {
        Ok(n) if n > 0 => {
            tracing::info!("reconcile: soft-retired {} stale {} target(s)", n, channel)
        }
        Ok(_) => {}
        Err(e) => tracing::error!("reconcile targets failed for {}: {:?}", channel, e),
    }
    let gc_before = chrono::Utc::now().timestamp() - TARGET_RETIRE_GRACE_SECS;
    if let Err(e) = state.collector_targets.purge_retired(gc_before).await {
        tracing::error!("purge retired targets failed: {:?}", e);
    }

    // 2. Head fetch + gap detection per target. Pacing is global (the adaptive
    //    rate limiter in `head_fetch_target`'s `lane.run`), so there's no
    //    per-target backoff to consult — a failing target just retries next pass.
    for (id, target) in &target_ids {
        head_fetch_target(state, collector, id, target, since).await;
    }

    // 3. Drain open backfill jobs for the channel with leftover budget.
    if let Ok(jobs) = state
        .collector_targets
        .list_open_jobs_for_channel(channel)
        .await
    {
        for job in jobs {
            // Find the planned target the job belongs to (its request params).
            let Some((_, target)) = target_ids.iter().find(|(tid, _)| *tid == job.target_id) else {
                // Target no longer planned (monitor removed); leave the job
                // durable but don't drive it this cycle.
                continue;
            };
            drain_backfill_job(state, collector, &job, target, max_backfill_cutoff).await;
        }
    }

    // 4. Aggregate channel-level status from target states.
    aggregate_channel_status(state, channel).await
}

/// Result of a targeted pass: the channel-level degraded signal and the caught-up
/// watermark.
pub struct TargetedPassOutcome {
    /// `Some(summary)` when one or more targets are unhealthy (sticky error);
    /// becomes `channel_configs.error_message`.
    pub error_message: Option<String>,
    /// Min `confirmed_watermark` across targets — the channel is caught up only
    /// to here. `None` when no target has a watermark yet.
    pub min_watermark: Option<i64>,
}

/// Head fetch (newest pages) for one target: page forward from the head, store
/// new mentions immediately, bank nothing durable for the head walk itself but
/// advance the watermark / enqueue a gap job based on where the walk landed.
async fn head_fetch_target(
    state: &Arc<AppState>,
    collector: &dyn TargetedCollector,
    target_id: &str,
    target: &PlannedTarget,
    since: Option<i64>,
) {
    let prev_watermark = state
        .collector_targets
        .get_target(target_id)
        .await
        .ok()
        .flatten()
        .and_then(|t| t.confirmed_watermark);

    let mut cursor: Option<String> = None;
    let mut deepest_oldest: Option<i64> = None;
    let mut reached_end = false;
    let mut seen: HashSet<String> = HashSet::new();
    let mut any_page = false;

    for page_idx in 0..MAX_HEAD_PAGES_PER_CYCLE {
        let lane = state.throttles.lane(target.lane.clone());
        // Acquire a token (AIMD-paced), run the fetch, report the outcome.
        let page = match lane
            .run(|| async {
                let page = collector
                    .fetch_target_page(target, &state.http, cursor.as_deref())
                    .await;
                Ok::<_, std::convert::Infallible>((page.outcome.clone(), page))
            })
            .await
        {
            Ok(page) => page,
            Err(_unreachable) => return,
        };

        match &page.outcome {
            Outcome::Throttled { .. } => {
                // Banked nothing this page; record the sticky failure and stop.
                // AIMD has already cut the global rate; we retry next pass.
                record_failure(state, target_id, "rate limited (HTTP 429)").await;
                return;
            }
            Outcome::Failure => {
                if page_idx == 0 {
                    // Surface the collector's real error (timeout / reset / HTTP
                    // status / body-read) rather than a generic "fetch failed".
                    let detail = page.error.as_deref().unwrap_or("fetch failed");
                    record_failure(state, target_id, detail).await;
                    return;
                }
                // Past page 1: keep the partial head result.
                break;
            }
            Outcome::Success => {}
        }

        any_page = true;
        // Store new mentions for this page immediately (keep the feed current).
        // Items older than the backfill floor (`since`) are never stored.
        store_page_mentions(state, collector.channel(), &page, since).await;

        let page_oldest = oldest_ts(&page.items);
        if let Some(o) = page_oldest {
            deepest_oldest = Some(deepest_oldest.map_or(o, |d: i64| d.min(o)));
        }

        // Dedup-driven stop: if the page brought no new ids, the source is
        // repeating — stop to avoid a loop.
        let mut new_on_page = 0usize;
        for it in &page.items {
            if seen.insert(it.external_id.clone()) {
                new_on_page += 1;
            }
        }

        // Stop conditions: reached watermark, past `since`, no cursor, no new.
        if page.next_cursor.is_none() || new_on_page == 0 {
            reached_end = page.next_cursor.is_none();
            break;
        }
        if let (Some(prev), Some(po)) = (prev_watermark, page_oldest) {
            if po <= prev {
                reached_end = true; // walked back to the confirmed point
                break;
            }
        }
        if let (Some(s), Some(po)) = (since, page_oldest) {
            if po < s {
                reached_end = true; // reached the backfill cutoff
                break;
            }
        }
        cursor = page.next_cursor;
    }

    if !any_page {
        return;
    }

    // Gap detection: the deepest point reached across every page fetched this
    // cycle (not just page 1) vs the durable watermark — see `detect_gap`.
    if let Some((range_start, range_end)) = detect_gap(deepest_oldest, prev_watermark) {
        if let Err(e) = state
            .collector_targets
            .enqueue_job(target_id, range_start, range_end)
            .await
        {
            tracing::error!(
                "enqueue backfill job failed for {}: {:?}",
                target.descriptor,
                e
            );
        }
    }

    // Advance the watermark (only when the walk proved contiguity).
    let new_watermark = advance_watermark(deepest_oldest, prev_watermark, reached_end);
    if let Err(e) = state
        .collector_targets
        .record_target_success(target_id, new_watermark)
        .await
    {
        tracing::error!("record success failed for {}: {:?}", target.descriptor, e);
    }
}

/// Drain up to [`MAX_BACKFILL_PAGES_PER_CYCLE`] older pages of one open job,
/// banking the cursor + shrinking the range after each page. Completes the job
/// when the source runs out, abandons it when the window falls outside the
/// backfill cutoff.
async fn drain_backfill_job(
    state: &Arc<AppState>,
    collector: &dyn TargetedCollector,
    job: &crate::db::repos::traits::BackfillJob,
    target: &PlannedTarget,
    max_backfill_cutoff: i64,
) {
    if job_out_of_window(job.range_end, max_backfill_cutoff) {
        let _ = state
            .collector_targets
            .mark_job(
                &job.id,
                "abandoned",
                Some("range older than backfill window"),
            )
            .await;
        return;
    }

    let mut cursor = job.next_cursor.clone();
    let mut pages_done = job.pages_done;
    let mut range_end = job.range_end;
    let mut seen: HashSet<String> = HashSet::new();

    for _ in 0..MAX_BACKFILL_PAGES_PER_CYCLE {
        let lane = state.throttles.lane(target.lane.clone());
        let page = match lane
            .run(|| async {
                let page = collector
                    .fetch_target_page(target, &state.http, cursor.as_deref())
                    .await;
                Ok::<_, std::convert::Infallible>((page.outcome.clone(), page))
            })
            .await
        {
            Ok(page) => page,
            Err(_unreachable) => return,
        };

        match &page.outcome {
            Outcome::Throttled { .. } => {
                // Bank what we have; the job stays open and resumes next cycle.
                let _ = state
                    .collector_targets
                    .update_job_progress(&job.id, cursor.as_deref(), pages_done, range_end)
                    .await;
                return;
            }
            Outcome::Failure => {
                let _ = state
                    .collector_targets
                    .update_job_progress(&job.id, cursor.as_deref(), pages_done, range_end)
                    .await;
                return;
            }
            Outcome::Success => {}
        }

        // Store mentions and bank progress for THIS page before moving on.
        // Floor stored items at the backfill-window cutoff (never ingest items
        // older than the channel is willing to look back).
        store_page_mentions(state, collector.channel(), &page, Some(max_backfill_cutoff)).await;
        pages_done += 1;

        let page_oldest = oldest_ts(&page.items);
        if let Some(o) = page_oldest {
            range_end = range_end.min(o);
        }
        let mut new_on_page = 0usize;
        for it in &page.items {
            if seen.insert(it.external_id.clone()) {
                new_on_page += 1;
            }
        }

        // Bank the cursor + shrunk range immediately (crash-safe resume).
        let _ = state
            .collector_targets
            .update_job_progress(&job.id, page.next_cursor.as_deref(), pages_done, range_end)
            .await;

        // Completion: source exhausted, no new items, or we've covered the
        // window down to range_start.
        let covered = page_oldest.is_some_and(|o| o <= job.range_start);
        if page.next_cursor.is_none() || new_on_page == 0 || covered {
            let _ = state.collector_targets.complete_job(&job.id).await;
            // The job's window is now contiguously ingested; the next head walk
            // will advance the watermark across it.
            return;
        }
        // Abandon if we've paged out of the backfill window.
        if job_out_of_window(range_end, max_backfill_cutoff) {
            let _ = state
                .collector_targets
                .mark_job(&job.id, "abandoned", Some("paged past backfill window"))
                .await;
            return;
        }
        cursor = page.next_cursor;
    }
}

/// Store the per-monitor mentions of one page: dedup via `mentions.exists`,
/// insert, and SSE-broadcast (mirrors `run_pass`'s storage, incl. AI gating).
/// `floor` (when set) drops items published before it — the backfill-window /
/// `since` cutoff — so a poll/backfill never ingests items older than the
/// channel is willing to look back. Dedup keeps re-fetched windows idempotent.
async fn store_page_mentions(
    state: &Arc<AppState>,
    channel: &str,
    page: &TargetPage,
    floor: Option<i64>,
) {
    for (monitor_id, raws) in &page.mentions {
        // Resolve the monitor for AI gating; a missing monitor just skips gating.
        let monitor = state.monitors.get(monitor_id).await.ok().flatten();
        let ai_gated = state.ai_judge().is_some()
            && monitor
                .as_ref()
                .and_then(|m| m.ai_filter_prompt.as_deref())
                .is_some_and(|p| !p.trim().is_empty());

        for raw in raws {
            // Time-window floor: drop items older than the cutoff.
            if let (Some(floor), Some(ts)) = (floor, raw.published_at) {
                if ts < floor {
                    continue;
                }
            }
            match state
                .mentions
                .exists(monitor_id, channel, &raw.external_id)
                .await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    tracing::error!("exists check failed: {:?}", e);
                    continue;
                }
            }
            let new_mention = crate::db::repos::traits::NewMention {
                monitor_id: monitor_id.clone(),
                channel: channel.to_string(),
                external_id: raw.external_id.clone(),
                content_text: raw.content_text.clone(),
                content_url: raw.content_url.clone(),
                author_name: raw.author_name.clone(),
                author_url: raw.author_url.clone(),
                published_at: raw.published_at,
                platform_meta: raw.platform_meta.clone(),
                ai_verdict: ai_gated.then(|| "pending".to_string()),
            };
            match state.mentions.insert(new_mention).await {
                Ok(mention) => {
                    if !ai_gated {
                        if let Ok(json) = serde_json::to_string(&mention) {
                            let _ = state.sse_tx.send(json);
                        }
                    }
                }
                Err(e) => tracing::error!("insert mention {} failed: {:?}", raw.external_id, e),
            }
        }
    }
}

/// Record a sticky target failure (display/health only). Pacing is handled by
/// the adaptive rate limiter, so there is no per-target backoff window: the next
/// poll cycle simply retries, AIMD-paced.
async fn record_failure(state: &Arc<AppState>, target_id: &str, error: &str) {
    if let Err(e) = state
        .collector_targets
        .record_target_failure(target_id, error)
        .await
    {
        tracing::error!("record failure failed: {:?}", e);
    }
}

/// Derive the channel-level degraded signal and caught-up watermark from the
/// target rows. The summary reflects each target's *current* sticky status
/// (only that target's own success clears its error), via [`summarize_targets`].
async fn aggregate_channel_status(state: &Arc<AppState>, channel: &str) -> TargetedPassOutcome {
    let targets = state
        .collector_targets
        .list_targets(channel)
        .await
        .unwrap_or_default();

    // Same classifier the per-target API view uses, so the stored channel banner
    // can't disagree with the per-target table.
    let error_message = summarize_targets(&targets).message;

    // Channel is caught up only as far as the LEAST caught-up target. If any
    // target has no watermark yet, the channel isn't fully caught up → None.
    //
    // The first two arms both evaluate to `None` and clippy flags them as
    // identical, but they're kept separate on purpose: they're two distinct
    // preconditions ("no targets planned yet" vs. "a planned target hasn't
    // succeeded once") that happen to share an outcome today. Collapsing them
    // into `targets.is_empty() || targets.iter().any(...)` would read as one
    // condition and invite someone to quietly change the merged branch's
    // outcome for both cases at once, when they may need to diverge later.
    #[allow(clippy::if_same_then_else)]
    let min_watermark = if targets.is_empty() {
        None
    } else if targets.iter().any(|t| t.confirmed_watermark.is_none()) {
        None
    } else {
        targets.iter().filter_map(|t| t.confirmed_watermark).min()
    };

    TargetedPassOutcome {
        error_message,
        min_watermark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn reddit_base_local_mock_detection() {
        std::env::remove_var("REDDIT_API_BASE");
        assert!(
            !reddit_base_is_local_test_mock(),
            "unset must not be treated as a local mock"
        );

        // What every test mock server actually looks like: 127.0.0.1:<port>.
        std::env::set_var("REDDIT_API_BASE", "http://127.0.0.1:54321");
        assert!(reddit_base_is_local_test_mock());
        std::env::set_var("REDDIT_API_BASE", "http://localhost:54321");
        assert!(reddit_base_is_local_test_mock());

        // A real deploy legitimately overriding REDDIT_API_BASE to route
        // through a self-hosted mirror or corporate proxy must NOT get
        // throttling disabled — this is the prod escape-hatch bug.
        std::env::set_var("REDDIT_API_BASE", "https://reddit-mirror.example.com");
        assert!(!reddit_base_is_local_test_mock());
        std::env::set_var("REDDIT_API_BASE", "https://reddit.com");
        assert!(!reddit_base_is_local_test_mock());

        std::env::remove_var("REDDIT_API_BASE");
    }

    fn item(id: &str, ts: Option<i64>) -> ParsedItem {
        ParsedItem {
            external_id: id.into(),
            published_at: ts,
        }
    }

    #[test]
    fn outcome_mapping_from_status() {
        assert!(matches!(
            outcome_for_status(Some(200), None),
            Outcome::Success
        ));
        assert!(matches!(
            outcome_for_status(Some(204), None),
            Outcome::Success
        ));
        assert!(matches!(
            outcome_for_status(Some(429), Some(Duration::from_secs(5))),
            Outcome::Throttled { retry_after: Some(d) } if d == Duration::from_secs(5)
        ));
        assert!(matches!(
            outcome_for_status(Some(500), None),
            Outcome::Failure
        ));
        assert!(matches!(
            outcome_for_status(Some(403), None),
            Outcome::Failure
        ));
        // Transport error (no status) → Failure.
        assert!(matches!(outcome_for_status(None, None), Outcome::Failure));
    }

    #[test]
    fn retry_after_seconds_only() {
        assert_eq!(parse_retry_after(Some("12")), Some(Duration::from_secs(12)));
        assert_eq!(
            parse_retry_after(Some("  7 ")),
            Some(Duration::from_secs(7))
        );
        // HTTP-date form is not parsed.
        assert_eq!(
            parse_retry_after(Some("Fri, 12 Jun 2026 09:00:00 GMT")),
            None
        );
        assert_eq!(parse_retry_after(None), None);
    }

    #[test]
    fn oldest_newest_ignore_missing_ts() {
        let items = [item("a", Some(100)), item("b", None), item("c", Some(50))];
        assert_eq!(oldest_ts(&items), Some(50));
        assert_eq!(newest_ts(&items), Some(100));
        assert_eq!(oldest_ts(&[item("x", None)]), None);
    }

    #[test]
    fn gap_detected_when_deepest_newer_than_watermark() {
        // The deepest point reached this cycle (200) is newer than the watermark
        // (100) → hole [100,200).
        assert_eq!(detect_gap(Some(200), Some(100)), Some((100, 200)));
        // The walk reached back under the watermark → contiguous, no gap.
        assert_eq!(detect_gap(Some(80), Some(100)), None);
        assert_eq!(detect_gap(Some(100), Some(100)), None);
        // No prior watermark (first run) → no gap job; head walk establishes it.
        assert_eq!(detect_gap(Some(200), None), None);
        // No timestamps at all → can't detect.
        assert_eq!(detect_gap(None, Some(100)), None);
    }

    #[test]
    fn gap_detection_uses_deepest_page_not_just_page1() {
        // Page 1's oldest was 200 (newer than the watermark of 100), but the same
        // cycle paged deeper and reached 90 — below the watermark. Gap detection
        // must be evaluated against the deepest point reached (90), which already
        // proves contiguity, not page 1's oldest (200), which would falsely flag a
        // hole that a later page in the SAME cycle already closed.
        assert_eq!(detect_gap(Some(90), Some(100)), None);
    }

    #[test]
    fn watermark_advances_only_when_contiguous() {
        // First run, walked to the end → commit the oldest seen.
        assert_eq!(advance_watermark(Some(50), None, true), Some(50));
        // First run, did NOT reach the end → don't commit (more below the page).
        assert_eq!(advance_watermark(Some(50), None, false), None);
        // Have a watermark, walked back under it (contiguous) → advance to oldest.
        assert_eq!(advance_watermark(Some(80), Some(100), false), Some(80));
        assert_eq!(advance_watermark(Some(100), Some(100), true), Some(100));
        // Have a watermark, a gap remains above it, but more pages might still
        // close it later this cycle/next cycle → stay put.
        assert_eq!(advance_watermark(Some(200), Some(100), false), Some(100));
        // No new timestamps → unchanged.
        assert_eq!(advance_watermark(None, Some(100), true), Some(100));
    }

    #[test]
    fn watermark_unwedges_when_source_exhausted_before_reaching_prior_watermark() {
        // A gap remains above `prev` (200 > 100), but the walk exhausted the
        // source this cycle (reached_end = true) without ever reaching back to
        // 100 — e.g. items around ts=100 aged out of Reddit's search horizon and
        // will never reappear. Holding the watermark at 100 forever would wedge
        // it permanently (every future cycle repeats the same story). The
        // watermark must move forward to the oldest point still reachable (200)
        // instead of staying stuck.
        assert_eq!(advance_watermark(Some(200), Some(100), true), Some(200));
    }

    #[test]
    fn job_window_check() {
        // range_end older than/equal to cutoff → out of window.
        assert!(job_out_of_window(100, 100));
        assert!(job_out_of_window(50, 100));
        // range_end newer than cutoff → still in window.
        assert!(!job_out_of_window(200, 100));
    }

    fn tgt(
        consecutive_failures: i64,
        last_success_at: Option<i64>,
        last_error: Option<&str>,
    ) -> CollectorTarget {
        CollectorTarget {
            id: "tgt_x".into(),
            channel: "reddit".into(),
            kind: "search".into(),
            descriptor: "\"x\"".into(),
            confirmed_watermark: None,
            last_success_at,
            last_attempt_at: None,
            consecutive_failures,
            last_error: last_error.map(String::from),
            updated_at: 0,
        }
    }

    #[test]
    fn target_health_classification() {
        // No failures, has succeeded → healthy.
        assert_eq!(
            target_health(&tgt(0, Some(900), None)),
            TargetHealth::Healthy
        );
        // No failures, never succeeded → idle.
        assert_eq!(target_health(&tgt(0, None, None)), TargetHealth::Idle);
        // Failing with a 429 error → throttled (regardless of how long ago).
        assert_eq!(
            target_health(&tgt(2, Some(900), Some("rate limited (HTTP 429)"))),
            TargetHealth::Throttled
        );
        // Failing with a non-throttle error → failing.
        assert_eq!(
            target_health(&tgt(2, None, Some("fetch failed"))),
            TargetHealth::Failing
        );
    }

    #[test]
    fn summarize_matches_per_target_classification() {
        let targets = vec![
            tgt(0, Some(900), None),             // healthy
            tgt(2, Some(900), Some("HTTP 429")), // throttled
            tgt(1, None, Some("fetch failed")),  // failing
        ];
        let s = summarize_targets(&targets);
        assert_eq!(s.total, 3);
        assert_eq!(s.healthy, 1);
        assert_eq!(s.throttled, 1);
        assert_eq!(s.failing, 1);
        // The banner's degraded count equals the number of non-healthy/idle
        // targets — i.e. exactly what the per-target table renders as warnings.
        assert_eq!(s.degraded, 2);
        let degraded_in_table = targets
            .iter()
            .filter(|t| target_health(t).is_degraded())
            .count();
        assert_eq!(s.degraded, degraded_in_table);
        // Throttle present → message leads with "rate-limited".
        assert!(s.message.unwrap().contains("rate-limited"));

        // All healthy → no banner.
        assert_eq!(summarize_targets(&[tgt(0, Some(900), None)]).message, None);
    }
}
