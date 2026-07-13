use async_trait::async_trait;
use std::sync::Arc;

use crate::db::repos::traits::{Monitor, NewMention};
use crate::state::AppState;

pub mod github;
pub mod github_filter;
pub mod hackernews;
pub mod reddit;
pub mod rss_parse;
pub mod scheduler;

/// Marker error a collector returns when the upstream service rate-limited us
/// (HTTP 429) and in-fetch retries were exhausted. [`run_pass`] aborts the
/// remaining monitors for the pass when it sees this — continuing would only
/// hammer an already-throttled service and deepen the penalty.
#[derive(Debug)]
pub struct RateLimited {
    pub url: String,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rate limited (HTTP 429 Too Many Requests) by {}",
            self.url
        )
    }
}

impl std::error::Error for RateLimited {}

pub struct RawMention {
    pub external_id: String,
    pub content_text: String,
    pub content_url: String,
    pub author_name: Option<String>,
    pub author_url: Option<String>,
    pub published_at: Option<i64>,
    pub platform_meta: serde_json::Value,
}

/// A monitor paired with its effective (merged) credentials for one pass.
pub struct MonitorFetch<'a> {
    pub monitor: &'a Monitor,
    pub creds: serde_json::Value,
}

/// Per-monitor outcomes of a pass-level fetch (monitor id → result). A monitor
/// missing from the list was skipped (e.g. the pass aborted on a rate limit
/// before reaching it).
pub type MonitorResults = Vec<(String, anyhow::Result<Vec<RawMention>>)>;

#[async_trait]
pub trait Collector: Send + Sync {
    fn channel(&self) -> &'static str;
    async fn fetch(
        &self,
        monitor: &Monitor,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        since: Option<i64>,
    ) -> anyhow::Result<Vec<RawMention>>;

    /// Fetch for every monitor of one collection pass. The default drives
    /// [`Collector::fetch`] once per monitor, stopping early when a fetch
    /// reports [`RateLimited`] — the remaining monitors would only hammer the
    /// same throttle. Collectors that can serve several monitors from shared
    /// requests (e.g. Reddit's OR-batched search) override this.
    async fn fetch_pass(
        &self,
        inputs: &[MonitorFetch<'_>],
        http: &reqwest::Client,
        since: Option<i64>,
    ) -> MonitorResults {
        let mut out = MonitorResults::new();
        for input in inputs {
            let res = self.fetch(input.monitor, http, &input.creds, since).await;
            let rate_limited = matches!(&res, Err(e) if e.downcast_ref::<RateLimited>().is_some());
            out.push((input.monitor.id.clone(), res));
            if rate_limited {
                tracing::warn!(
                    "Collector {} rate limited; skipping remaining monitors this pass",
                    self.channel()
                );
                break;
            }
        }
        out
    }

    /// Opt into the durable, auto-recovering [`scheduler`] runner. Collectors
    /// that return `Some(self)` are driven via target-based collection (head
    /// fetch + banked backfill + sticky status); the default `None` keeps the
    /// legacy [`Collector::fetch_pass`] path (HackerNews, GitHub). Reddit
    /// overrides this.
    fn as_targeted(&self) -> Option<&dyn scheduler::TargetedCollector> {
        None
    }
}

/// Default safety cap on how many pages a single `fetch` call will request when
/// backward-paginating toward `since`. Overridable via the
/// `COLLECTOR_MAX_BACKFILL_PAGES` env var. Bounds the worst case (first poll or
/// an explicit far-back backfill) so a collector can't run away or get
/// rate-limited.
pub const DEFAULT_MAX_BACKFILL_PAGES: u32 = 10;

/// Resolve the per-`fetch` page cap from `COLLECTOR_MAX_BACKFILL_PAGES`, falling
/// back to [`DEFAULT_MAX_BACKFILL_PAGES`]. A value of 0 in the env is treated as
/// the default (paginating zero pages would fetch nothing).
pub fn max_backfill_pages() -> u32 {
    std::env::var("COLLECTOR_MAX_BACKFILL_PAGES")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_BACKFILL_PAGES)
}

/// Pure stop-condition for backward pagination, shared by all collectors.
///
/// Returns `true` when the loop should fetch ANOTHER page after the one just
/// processed. `page_index` is 0-based for the page we just fetched.
///
/// Stops (returns `false`) when ANY of:
/// - the page yielded no NEW items (`new_items_on_page == 0`) — dedup/seen set
///   saw only repeats (e.g. a source that ignores the cursor), so paging again
///   is pointless and would loop forever;
/// - the oldest item on the page is older than `since` — we've reached back past
///   the cutoff, nothing deeper can be in-window;
/// - we've hit the page cap (`page_index + 1 >= max_pages`).
///
/// `oldest_ts_on_page` is the oldest published timestamp seen on the page (if
/// any item carried one). When `None` (no timestamps) the time check is skipped
/// and we rely on the new-items / cap conditions to terminate.
pub fn should_continue_paging(
    new_items_on_page: usize,
    oldest_ts_on_page: Option<i64>,
    since: Option<i64>,
    page_index: u32,
    max_pages: u32,
) -> bool {
    if new_items_on_page == 0 {
        return false;
    }
    if page_index + 1 >= max_pages {
        return false;
    }
    if let (Some(since_ts), Some(oldest)) = (since, oldest_ts_on_page) {
        if oldest < since_ts {
            return false;
        }
    }
    true
}

/// Effective per-fetch settings: the channel's global credentials with this
/// monitor's `channel_settings[channel]` shallow-merged on top (monitor keys
/// win). Lets scoping like Reddit subreddits or GitHub repo filters live on
/// the monitor while the global channel config stays empty.
pub fn merged_credentials(
    global: &serde_json::Value,
    monitor_overrides: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut base = match global {
        serde_json::Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    if let Some(serde_json::Value::Object(overrides)) = monitor_overrides {
        for (k, v) in overrides {
            base.insert(k.clone(), v.clone());
        }
    }
    serde_json::Value::Object(base)
}

/// A post matches a monitor iff it contains ANY of the monitor's `terms` (each
/// applied per `exact_match` / `case_sensitive`) AND contains NONE of its
/// `exclude_terms`. Each term is a bare literal phrase — there is no OR/quote
/// parsing inside a term; "OR" happens across the term list. A monitor with no
/// terms matches nothing.
pub fn matches_monitor(monitor: &Monitor, text: &str) -> bool {
    let haystack = if monitor.case_sensitive {
        text.to_string()
    } else {
        text.to_lowercase()
    };

    let norm = |s: &str| {
        if monitor.case_sensitive {
            s.to_string()
        } else {
            s.to_lowercase()
        }
    };

    // Excludes veto regardless of which term matched.
    for excl in &monitor.exclude_terms {
        if haystack.contains(norm(excl).as_str()) {
            return false;
        }
    }

    monitor.terms.iter().any(|term| {
        let needle = norm(term);
        if needle.is_empty() {
            return false;
        }
        if monitor.exact_match {
            // `\b` only fires at a transition between a \w char and a non-\w
            // char, so a term whose first/last char is already non-word (like
            // `C++` or `.NET`) can never satisfy a `\b` on that edge — both
            // sides of the would-be boundary are non-word, so `\bC\+\+\b`
            // never matches "C++" anywhere. Only require the boundary on
            // edges where the term itself starts/ends with a word char.
            let is_word = |c: char| c.is_alphanumeric() || c == '_';
            let left = needle.chars().next().is_some_and(is_word);
            let right = needle.chars().next_back().is_some_and(is_word);
            let pat = format!(
                r"(?-i){}{}{}",
                if left { r"\b" } else { "" },
                regex::escape(&needle),
                if right { r"\b" } else { "" },
            );
            regex::Regex::new(&pat)
                .map(|re| re.is_match(&haystack))
                .unwrap_or(false)
        } else {
            haystack.contains(needle.as_str())
        }
    })
}

/// Percent-encode a query-string value the way each collector's search API
/// expects: unreserved chars pass through, a space becomes `+` (the common
/// `application/x-www-form-urlencoded` convention all three of Reddit's,
/// GitHub's, and HN/Algolia's search endpoints accept), everything else is
/// percent-escaped byte-by-byte. Shared by every collector that builds a
/// search URL so there is exactly one implementation to keep correct.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            ' ' => out.push('+'),
            c => {
                for byte in c.to_string().as_bytes() {
                    out.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    out
}

/// Every channel name a collector exists for. The CLI surfaces this list in
/// help text and validation; keep it in sync with [`make_collector`].
pub const CHANNELS: &[&str] = &["hackernews", "reddit", "github"];

/// Single factory for all collectors (core + extras). Returns `None` for an
/// unknown channel name.
pub fn make_collector(channel: &str) -> Option<Box<dyn Collector>> {
    match channel {
        "hackernews" => Some(Box::new(hackernews::HackerNewsCollector)),
        "reddit" => Some(Box::new(reddit::RedditCollector::new())),
        "github" => Some(Box::new(github::GitHubCollector)),
        _ => None,
    }
}

/// Spawn one background poll loop per channel — driven by [`CHANNELS`] /
/// [`make_collector`] (the single source of truth for "every channel a
/// collector exists for") rather than a third hand-written list that could
/// drift from the factory and the CLI's channel validation.
pub fn spawn_all(state: Arc<AppState>) {
    for &channel in CHANNELS {
        let collector = make_collector(channel).unwrap_or_else(|| {
            panic!("CHANNELS lists '{channel}' but make_collector returns None")
        });
        let state = state.clone();
        tokio::spawn(async move {
            run_collector(state, collector).await;
        });
    }
}

pub async fn run_once(state: &Arc<AppState>, channel: &str) {
    if let Some(collector) = make_collector(channel) {
        run_pass(state, collector.as_ref(), None).await;
    }
}

pub async fn run_once_since(state: &Arc<AppState>, channel: &str, since: i64) {
    if let Some(collector) = make_collector(channel) {
        run_pass(state, collector.as_ref(), Some(since)).await;
    }
}

pub async fn run_collector(state: Arc<AppState>, collector: Box<dyn Collector>) {
    let channel = collector.channel();
    tracing::info!("Starting collector for channel: {}", channel);
    loop {
        let sleep_secs = run_pass(&state, collector.as_ref(), None)
            .await
            .unwrap_or(60);
        tokio::time::sleep(tokio::time::Duration::from_secs(sleep_secs)).await;
    }
}

// Performs one collection pass. Returns the poll interval on success, None if skipped.
async fn run_pass(
    state: &Arc<AppState>,
    collector: &dyn Collector,
    since_override: Option<i64>,
) -> Option<u64> {
    let channel = collector.channel();

    let config = match state.channels.get(channel).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            tracing::debug!("No config for channel {}, skipping", channel);
            return None;
        }
        Err(e) => {
            tracing::error!("Error getting channel config for {}: {:?}", channel, e);
            return None;
        }
    };

    if !config.enabled {
        return None;
    }

    let poll_interval = config.poll_interval as u64;

    // If a since_override is provided, use it directly; otherwise compute from config.
    let since: Option<i64> = if let Some(ts) = since_override {
        Some(ts)
    } else {
        // Compute the backfill cutoff: how far back we're willing to look
        let max_backfill_secs = config.max_backfill_days * 86_400;
        let cutoff = chrono::Utc::now().timestamp() - max_backfill_secs;
        Some(config.caught_up_at.map(|t| t.max(cutoff)).unwrap_or(cutoff))
    };

    let monitors = match state.monitors.list_active_all().await {
        Ok(kws) => kws,
        Err(e) => {
            tracing::error!("Error getting monitors: {:?}", e);
            return Some(poll_interval);
        }
    };

    let relevant_monitors: Vec<_> = monitors
        .iter()
        .filter(|kw| kw.channels.is_empty() || kw.channels.contains(&channel.to_string()))
        .collect();

    let inputs: Vec<MonitorFetch<'_>> = relevant_monitors
        .iter()
        .map(|m| MonitorFetch {
            monitor: m,
            creds: merged_credentials(&config.credentials, m.channel_settings.get(channel)),
        })
        .collect();

    // Targeted collectors (Reddit) take the durable, auto-recovering runner:
    // head fetch + banked backfill + sticky per-target status. The channel's
    // `caught_up_at` advances only to the minimum confirmed watermark across
    // targets (caught up == all targets caught up), and `error_message` carries
    // a degraded/rate-limited summary derived from target state.
    if let Some(targeted) = collector.as_targeted() {
        let max_backfill_cutoff =
            chrono::Utc::now().timestamp() - config.max_backfill_days * 86_400;
        let outcome =
            scheduler::run_targeted_pass(state, targeted, &inputs, since, max_backfill_cutoff)
                .await;
        if let Err(e) = state
            .channels
            .update_polled(channel, outcome.error_message.as_deref())
            .await
        {
            tracing::error!("Error updating polled time for {}: {:?}", channel, e);
        }
        // Only advance caught_up_at to the channel's caught-up point.
        if outcome.error_message.is_none() {
            if let Err(e) = state
                .channels
                .set_caught_up_at(channel, outcome.min_watermark)
                .await
            {
                tracing::error!("Error updating fetched time for {}: {:?}", channel, e);
            }
        }
        return Some(poll_interval);
    }

    let results = collector.fetch_pass(&inputs, &state.http, since).await;

    let mut error_message: Option<String> = None;

    for (monitor_id, result) in results {
        let Some(monitor) = relevant_monitors
            .iter()
            .find(|m| m.id == monitor_id)
            .copied()
        else {
            continue;
        };
        // New mentions from a monitor with an AI prompt are held back
        // (ai_verdict = 'pending') until the AI filter worker accepts them;
        // they are not broadcast to the feed here. Without a wired judge the
        // gate is skipped so mentions are never held back forever.
        let ai_gated = state.ai_judge().is_some()
            && monitor
                .ai_filter_prompt
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty());

        match result {
            Ok(raw_mentions) => {
                for raw in raw_mentions {
                    let exists = match state
                        .mentions
                        .exists(&monitor.id, channel, &raw.external_id)
                        .await
                    {
                        Ok(e) => e,
                        Err(e) => {
                            tracing::error!("Error checking mention existence: {:?}", e);
                            continue;
                        }
                    };

                    if exists {
                        continue;
                    }

                    let new_mention = NewMention {
                        monitor_id: monitor.id.clone(),
                        channel: channel.to_string(),
                        external_id: raw.external_id.clone(),
                        content_text: raw.content_text,
                        content_url: raw.content_url,
                        author_name: raw.author_name,
                        author_url: raw.author_url,
                        published_at: raw.published_at,
                        platform_meta: raw.platform_meta,
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
                        Err(e) => {
                            tracing::error!("Error inserting mention {}: {:?}", raw.external_id, e);
                        }
                    }
                }
            }
            Err(e) => {
                // Rate-limit early-stopping happens inside fetch_pass; here we
                // only record the failure for the channel status.
                tracing::warn!(
                    "Collector {} error for monitor {:?}: {:?}",
                    channel,
                    monitor.terms,
                    e
                );
                error_message = Some(e.to_string());
            }
        }
    }

    if let Err(e) = state
        .channels
        .update_polled(channel, error_message.as_deref())
        .await
    {
        tracing::error!("Error updating polled time for {}: {:?}", channel, e);
    }

    // Only update caught_up_at when the full pass completed without errors
    if error_message.is_none() {
        if let Err(e) = state.channels.set_caught_up_now(channel).await {
            tracing::error!("Error updating fetched time for {}: {:?}", channel, e);
        }
    }

    Some(poll_interval)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_passes_unreserved_and_escapes_the_rest() {
        assert_eq!(percent_encode("nimbusdb"), "nimbusdb");
        assert_eq!(percent_encode("a b"), "a+b");
        assert_eq!(percent_encode(r#""a b" OR "c""#), "%22a+b%22+OR+%22c%22");
        assert_eq!(percent_encode("C++"), "C%2B%2B");
    }

    const CAP: u32 = 10;

    fn monitor_with(terms: &[&str]) -> Monitor {
        Monitor {
            id: "m1".into(),
            workspace_id: "w1".into(),
            terms: terms.iter().map(|s| s.to_string()).collect(),
            active: true,
            channels: vec![],
            exact_match: false,
            case_sensitive: false,
            exclude_terms: vec![],
            channel_settings: serde_json::Value::Object(Default::default()),
            ai_filter_prompt: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn matches_any_term() {
        let m = monitor_with(&["FlaUI", "Ranorex", "WinAppDriver"]);
        assert!(matches_monitor(&m, "I used Ranorex for this"));
        assert!(matches_monitor(&m, "WinAppDriver is great"));
        assert!(!matches_monitor(&m, "nothing relevant here"));
    }

    #[test]
    fn no_terms_matches_nothing() {
        let m = monitor_with(&[]);
        assert!(!matches_monitor(&m, "anything at all"));
    }

    #[test]
    fn substring_vs_exact_match() {
        let mut m = monitor_with(&["test"]);
        // substring: matches inside a word
        assert!(matches_monitor(&m, "this is the latest news"));
        // exact: word boundaries only
        m.exact_match = true;
        assert!(!matches_monitor(&m, "this is the latest news"));
        assert!(matches_monitor(&m, "run the test now"));
    }

    #[test]
    fn exact_match_matches_terms_with_symbol_edges() {
        // `C++` starts and ends with a non-word char (`+`), so a bare `\b`
        // on both edges could never match; the fix only requires a boundary
        // on edges where the term itself starts/ends with a word char.
        let mut m = monitor_with(&["C++"]);
        m.exact_match = true;
        assert!(matches_monitor(&m, "we ship C++ on the backend"));
        assert!(matches_monitor(&m, "C++ is the whole codebase"));
        // still respects the boundary on the word-char side: `Cx++` isn't `C++`.
        assert!(!matches_monitor(&m, "Cx++ is not the same term"));

        // `.NET` starts with a non-word char (`.`) and ends with a word char.
        let mut m = monitor_with(&[".NET"]);
        m.exact_match = true;
        assert!(matches_monitor(&m, "we use .NET for this service"));
        assert!(matches_monitor(&m, ".NET"));
        // the trailing word-char boundary still applies: `.NETting` isn't `.NET`.
        assert!(!matches_monitor(&m, "no .NETting here"));
    }

    #[test]
    fn excludes_veto_any_term_match() {
        let mut m = monitor_with(&["selenium", "playwright"]);
        m.exclude_terms = vec!["hiring".into()];
        assert!(matches_monitor(&m, "loving playwright lately"));
        // exclude term present -> dropped even though a term matched
        assert!(!matches_monitor(&m, "playwright role, hiring now"));
    }

    #[test]
    fn case_sensitivity() {
        let mut m = monitor_with(&["Pulp"]);
        // default: case-insensitive
        assert!(matches_monitor(&m, "love pulp"));
        m.case_sensitive = true;
        assert!(!matches_monitor(&m, "love pulp"));
        assert!(matches_monitor(&m, "love Pulp"));
    }

    #[test]
    fn stops_when_no_new_items() {
        // A source that ignored the cursor and repeated the page → 0 new items.
        assert!(!should_continue_paging(0, Some(100), Some(0), 0, CAP));
    }

    #[test]
    fn stops_when_oldest_older_than_since() {
        // Oldest item on the page (ts=50) is older than the cutoff (since=100):
        // we've reached back past the window, stop.
        assert!(!should_continue_paging(5, Some(50), Some(100), 0, CAP));
    }

    #[test]
    fn continues_when_in_window_and_under_cap() {
        // Oldest item (ts=200) is still newer than `since` (100), new items on
        // the page, and we're under the cap → keep paging.
        assert!(should_continue_paging(5, Some(200), Some(100), 0, CAP));
    }

    #[test]
    fn stops_at_page_cap() {
        // Page index 9 is the 10th page; cap is 10 → must not request an 11th
        // even though items are in-window and new.
        assert!(!should_continue_paging(
            5,
            Some(9_999_999_999),
            Some(100),
            9,
            CAP
        ));
        // One before the cap still continues.
        assert!(should_continue_paging(
            5,
            Some(9_999_999_999),
            Some(100),
            8,
            CAP
        ));
    }

    #[test]
    fn steady_state_normal_poll_stops_after_one_page() {
        // Steady-state poll: `since ≈ caught_up_at` (recent). Page 1's oldest
        // item is already older than `since`, so we stop after the first page.
        // since=1000, oldest item on page = 900 (< since) → stop.
        assert!(!should_continue_paging(5, Some(900), Some(1000), 0, CAP));
    }

    #[test]
    fn no_timestamps_relies_on_new_items_and_cap() {
        // When the page carries no timestamps, the time check is skipped; we keep
        // going on new items until the cap.
        assert!(should_continue_paging(5, None, Some(100), 0, CAP));
        assert!(!should_continue_paging(0, None, Some(100), 0, CAP));
        assert!(!should_continue_paging(5, None, Some(100), 9, CAP));
    }

    #[test]
    fn since_none_never_stops_on_time() {
        // No `since` floor (e.g. open-ended) → never stop on the time condition,
        // only on new-items / cap.
        assert!(should_continue_paging(5, Some(0), None, 0, CAP));
    }

    /// `CHANNELS` (what the CLI advertises/validates) must agree with the
    /// factory: every listed name builds a collector that reports that name.
    #[test]
    fn channels_list_matches_factory() {
        for &name in CHANNELS {
            let collector = make_collector(name).unwrap_or_else(|| {
                panic!("CHANNELS lists '{name}' but make_collector returns None")
            });
            assert_eq!(collector.channel(), name);
        }
        assert!(make_collector("not-a-channel").is_none());
    }

    #[test]
    fn merged_credentials_monitor_overrides_win() {
        let global = serde_json::json!({ "user_agent": "ua", "subreddits": ["old"] });
        let overrides = serde_json::json!({ "subreddits": ["a11y", "rust"] });
        let merged = merged_credentials(&global, Some(&overrides));
        assert_eq!(merged["user_agent"], "ua");
        assert_eq!(merged["subreddits"], serde_json::json!(["a11y", "rust"]));
    }

    #[test]
    fn merged_credentials_handles_empty_global() {
        // The whole point: a monitor can scope a channel even when the global
        // channel config carries no settings at all.
        let merged = merged_credentials(
            &serde_json::json!({}),
            Some(&serde_json::json!({ "subreddits": ["a11y"] })),
        );
        assert_eq!(merged["subreddits"], serde_json::json!(["a11y"]));

        let untouched = merged_credentials(&serde_json::json!({ "k": 1 }), None);
        assert_eq!(untouched, serde_json::json!({ "k": 1 }));

        // Non-object inputs degrade gracefully instead of panicking.
        let odd = merged_credentials(&serde_json::json!("nope"), Some(&serde_json::json!(42)));
        assert_eq!(odd, serde_json::json!({}));
    }

    #[test]
    fn max_backfill_pages_default_and_env() {
        // Default when unset.
        std::env::remove_var("COLLECTOR_MAX_BACKFILL_PAGES");
        assert_eq!(max_backfill_pages(), DEFAULT_MAX_BACKFILL_PAGES);
        std::env::set_var("COLLECTOR_MAX_BACKFILL_PAGES", "3");
        assert_eq!(max_backfill_pages(), 3);
        // 0 / garbage fall back to the default (paginating 0 pages fetches nothing).
        std::env::set_var("COLLECTOR_MAX_BACKFILL_PAGES", "0");
        assert_eq!(max_backfill_pages(), DEFAULT_MAX_BACKFILL_PAGES);
        std::env::set_var("COLLECTOR_MAX_BACKFILL_PAGES", "nope");
        assert_eq!(max_backfill_pages(), DEFAULT_MAX_BACKFILL_PAGES);
        std::env::remove_var("COLLECTOR_MAX_BACKFILL_PAGES");
    }
}
