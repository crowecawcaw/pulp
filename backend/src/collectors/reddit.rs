use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::collectors::rss_parse::{extract_attr, extract_items, extract_tag, strip_html};
use crate::collectors::scheduler::{
    outcome_for_status, parse_retry_after, ParsedItem, PlannedTarget, TargetKind, TargetPage,
    TargetRequest, TargetedCollector,
};
use crate::collectors::{
    matches_monitor, max_backfill_pages, percent_encode, should_continue_paging, Collector,
    MonitorFetch, RawMention,
};
use crate::db::repos::traits::Monitor;
use crate::ratelimit::Outcome;

/// Reddit collector.
///
/// Uses Reddit's **public, unauthenticated RSS/Atom** endpoints (`*.rss`) rather
/// than the `*.json` API. Reddit's JSON search endpoints return `403 Forbidden`
/// to unauthenticated clients regardless of `User-Agent`, but the RSS feeds are
/// served openly. No OAuth app, client id, or secret is required.
///
/// ## Credentials schema (all optional)
/// ```json
/// {
///   "user_agent": "pulp:v0.1 (by /u/you)",
///   "subreddits": ["QualityAssurance", "softwaretesting"],
///   "exclude_subreddits": ["deals"],
///   "exclude_authors": ["spammer"]
/// }
/// ```
///
/// ## Fetch model — global search only (request volume is the design constraint)
/// Every monitor is served by Reddit's global `search.rss?q="a" OR "b" OR …`.
/// In a collection pass ALL monitors' terms are OR-batched into one query (or a
/// few, chunked under Reddit's query-length cap) and each result is matched
/// client-side against every monitor.
///
/// `subreddits` is a **client-side include filter** on those results (empty =
/// all of Reddit), `exclude_subreddits` / `exclude_authors` are exclude filters.
/// We deliberately do NOT use the multireddit `new.rss` firehose or per-sub
/// `restrict_sr` search: both multiply request count against Reddit's per-IP
/// limit and the firehose is the surface Reddit throttles hardest. The trade-off
/// is that search is indexed (a new post appears with some latency, vs instantly
/// in a firehose) and not perfectly exhaustive — acceptable for keyword
/// listening on distinctive terms.
///
/// ## Caveat
/// Reddit RSS carries no `score` / `num_comments`, so those fields are omitted
/// from `platform_meta` (no `score` key). Criteria that filter on `score` can no
/// longer match Reddit mentions.
/// A target's member monitors plus each one's parsed Reddit-specific settings,
/// keyed by the target's `(kind, descriptor)`. Named alias purely to keep
/// [`RedditCollector::plan`]'s type signature readable.
type TargetPlan = HashMap<(String, String), Vec<(Monitor, RedditSettings)>>;

pub struct RedditCollector {
    /// Per-pass plan state stashed by [`TargetedCollector::plan_targets`] and
    /// read back by [`TargetedCollector::fetch_target_page`] / mention mapping.
    /// A fresh collector instance is built per pass (`make_collector`/
    /// `spawn_all`), so this never leaks across passes. Each entry holds the
    /// member monitors + their parsed settings so the page's entries can be
    /// matched client-side per monitor.
    plan: Mutex<TargetPlan>,
}

impl Default for RedditCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl RedditCollector {
    pub fn new() -> Self {
        Self {
            plan: Mutex::new(HashMap::new()),
        }
    }

    fn api_base() -> String {
        std::env::var("REDDIT_API_BASE").unwrap_or_else(|_| "https://www.reddit.com".to_string())
    }
}

#[derive(Deserialize, Default, Clone)]
struct RedditSettings {
    #[serde(default)]
    user_agent: Option<String>,
    /// Client-side subreddit INCLUDE filter on global-search results (empty =
    /// all of Reddit). Formerly drove per-sub search / the multireddit firehose;
    /// now purely a client-side filter. (Unknown legacy keys like `mode` /
    /// `global_search` left in stored creds are ignored by serde.)
    #[serde(default)]
    subreddits: Vec<String>,
    #[serde(default)]
    exclude_subreddits: Vec<String>,
    #[serde(default)]
    exclude_authors: Vec<String>,
}

impl RedditSettings {
    fn user_agent(&self) -> &str {
        // An empty/whitespace user_agent (e.g. a `""` left in the channel
        // credentials by the UI) must fall back too: Reddit throttles blank-UA
        // clients far more aggressively than identified ones. The default is a
        // placeholder; operators should override it with their own agent.
        self.user_agent
            .as_deref()
            .map(str::trim)
            .filter(|ua| !ua.is_empty())
            .unwrap_or("pulp-social-listening/0.1")
    }
}

/// A parsed Atom entry from a Reddit feed, before monitor/filter checks.
struct ParsedEntry {
    external_id: String,
    content_text: String,
    content_url: String,
    author: Option<String>,
    subreddit: String,
    kind: &'static str,
    published_at: Option<i64>,
}

/// Parse a Reddit Atom feed body into entries (no monitor/exclude/time filtering).
fn parse_feed(body: &str) -> Vec<ParsedEntry> {
    let mut out = Vec::new();
    for item in extract_items(body) {
        let title = extract_tag(&item, "title").unwrap_or_default();
        let content_raw = extract_tag(&item, "content")
            .or_else(|| extract_tag(&item, "summary"))
            .unwrap_or_default();
        let content = strip_html(&content_raw);

        // Permalink: Atom <link href="..."/>.
        let link = extract_attr(&item, "link", "href")
            .or_else(|| extract_tag(&item, "link"))
            .unwrap_or_default();

        // Thing id: <id> is e.g. `t3_abc123` or a tag URI ending in it.
        let raw_id = extract_tag(&item, "id").unwrap_or_default();
        let (external_id, kind) = thing_id(&raw_id, &link);
        if external_id.is_empty() {
            continue;
        }

        // Subreddit: <category term="sub" .../> with permalink fallback.
        let subreddit = extract_attr(&item, "category", "term")
            .filter(|s| !s.is_empty())
            .or_else(|| subreddit_from_permalink(&link))
            .unwrap_or_default();

        // Author: <author><name>/u/username</name></author>; strip /u/.
        let author = extract_tag(&item, "name")
            .or_else(|| extract_tag(&item, "author"))
            .map(|a| {
                a.trim_start_matches("/u/")
                    .trim_start_matches("u/")
                    .to_string()
            })
            .filter(|a| !a.is_empty());

        let published_at = extract_tag(&item, "updated")
            .or_else(|| extract_tag(&item, "published"))
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp());

        let content_text = if content.trim().is_empty() {
            title.clone()
        } else {
            format!("{} {}", title, content).trim().to_string()
        };

        out.push(ParsedEntry {
            external_id,
            content_text,
            content_url: link,
            author,
            subreddit,
            kind,
            published_at,
        });
    }
    out
}

/// Derive the `t3_`/`t1_` thing-id and kind from the Atom `<id>` (or permalink).
fn thing_id(raw_id: &str, permalink: &str) -> (String, &'static str) {
    // The <id> often looks like `t3_abc123` or `tag:reddit.com,...:t3_abc123`.
    let candidate = raw_id.rsplit(':').next().unwrap_or(raw_id).trim();
    if let Some(rest) = candidate.strip_prefix("t3_") {
        if !rest.is_empty() {
            return (format!("t3_{}", rest), "link");
        }
    }
    if let Some(rest) = candidate.strip_prefix("t1_") {
        if !rest.is_empty() {
            return (format!("t1_{}", rest), "comment");
        }
    }
    // Fallback: pull the post id out of the permalink `/comments/<id>/`.
    if let Some(after) = permalink.split("/comments/").nth(1) {
        let id = after.split('/').next().unwrap_or("");
        if !id.is_empty() {
            return (format!("t3_{}", id), "link");
        }
    }
    (String::new(), "link")
}

/// Build the OR-batched global search query: each TERM quoted (Reddit treats a
/// quoted phrase as an exact phrase search, which matches the collector's own
/// literal-substring filter), joined with OR so one `search.rss` request serves
/// every unscoped monitor in a pass. Terms are flattened across all the
/// monitors' `terms` lists — each term is a bare literal, so quoting is correct
/// (no nested-quote/OR breakage). Duplicate terms are de-duplicated.
/// Join terms into one OR-batched query, each term quoted (Reddit treats a
/// quoted phrase as an exact-phrase search, matching the collector's own
/// literal-substring filter).
fn or_join(terms: &[&str]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{}\"", sanitize_for_quoting(t)))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Strip a term's embedded double quotes before it's wrapped in a quoted
/// phrase for the OR-batched query. An unescaped `"` inside a term would
/// otherwise prematurely close its phrase and corrupt the rest of the shared
/// query — poisoning the batch for every other monitor sharing this request.
fn sanitize_for_quoting(t: &str) -> String {
    t.replace('"', "")
}

/// Reddit's search `q` accepts only a bounded query string; keep each OR-batch
/// comfortably under this (chars of the un-encoded query). Terms beyond one
/// chunk's budget spill into additional global-search targets.
const MAX_QUERY_CHARS: usize = 512;

/// The OR-batched global search query for a set of terms (deduped + sorted so
/// term order can't churn the target's identity hash). Equivalent to the first
/// (and only) chunk of [`chunk_queries`] with an unbounded budget. Test-only
/// convenience — production planning goes through [`chunk_queries`].
#[cfg(test)]
fn global_or_query<'a>(terms: impl Iterator<Item = &'a str>) -> String {
    chunk_queries(terms, usize::MAX)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// Split `terms` into one or more OR-batched, quoted search queries, each at
/// most `max_chars` long (the un-encoded query text). Terms are deduped + sorted
/// first (canonical, so order can't churn target identity), then greedily packed.
/// A single term longer than `max_chars` still gets its own (over-length) chunk
/// rather than being dropped. Every term is searched across the chunks, and the
/// runner matches each chunk's results against every monitor, so splitting terms
/// across chunks never loses coverage.
fn chunk_queries<'a>(terms: impl Iterator<Item = &'a str>, max_chars: usize) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut uniq: Vec<&str> = terms
        .filter(|t| !t.trim().is_empty())
        .filter(|t| seen.insert(t.to_string()))
        .collect();
    uniq.sort_unstable();

    const SEP: usize = 4; // " OR "
    let quoted_len = |t: &str| t.len() + 2; // "t"

    let mut chunks: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut cur_len = 0usize;
    for t in uniq {
        let add = if cur.is_empty() {
            quoted_len(t)
        } else {
            SEP + quoted_len(t)
        };
        if !cur.is_empty() && cur_len + add > max_chars {
            chunks.push(or_join(&cur));
            cur.clear();
            cur_len = 0;
        }
        cur_len += if cur.is_empty() {
            quoted_len(t)
        } else {
            SEP + quoted_len(t)
        };
        cur.push(t);
    }
    if !cur.is_empty() {
        chunks.push(or_join(&cur));
    }
    chunks
}

/// All terms across a set of monitors, flattened — the input to
/// [`chunk_queries`] for a pass's global search.
fn flatten_terms<'a>(monitors: impl Iterator<Item = &'a Monitor>) -> Vec<&'a str> {
    monitors
        .flat_map(|m| m.terms.iter().map(String::as_str))
        .collect()
}

fn subreddit_from_permalink(permalink: &str) -> Option<String> {
    let after = permalink.split("/r/").nth(1)?;
    let sub = after.split('/').next()?;
    if sub.is_empty() {
        None
    } else {
        Some(sub.to_string())
    }
}

/// Apply monitor match, exclude lists, and the `since` time window to a parsed
/// entry, producing a `RawMention` if it survives. Takes the entry by reference
/// so one fetched feed can be matched against many monitors (target pages).
fn entry_to_mention(
    entry: &ParsedEntry,
    monitor: &Monitor,
    settings: &RedditSettings,
    since: Option<i64>,
) -> Option<RawMention> {
    if entry.content_text.is_empty() || !matches_monitor(monitor, &entry.content_text) {
        return None;
    }

    let sub_lc = entry.subreddit.to_lowercase();
    // Subreddit scope is a client-side INCLUDE filter on global search (we no
    // longer issue per-sub / firehose requests). Empty list = all of Reddit.
    if !settings.subreddits.is_empty()
        && !settings
            .subreddits
            .iter()
            .any(|s| s.trim().to_lowercase() == sub_lc)
    {
        return None;
    }
    if settings
        .exclude_subreddits
        .iter()
        .any(|s| s.to_lowercase() == sub_lc)
    {
        return None;
    }

    if let Some(author) = &entry.author {
        let author_lc = author.to_lowercase();
        if settings
            .exclude_authors
            .iter()
            .any(|a| a.to_lowercase() == author_lc)
        {
            return None;
        }
    }

    // Client-side time window for backfill.
    if let (Some(since_ts), Some(pub_ts)) = (since, entry.published_at) {
        if pub_ts < since_ts {
            return None;
        }
    }

    let author_url = entry
        .author
        .as_ref()
        .map(|a| format!("https://www.reddit.com/user/{}", a));

    let platform_meta = serde_json::json!({
        "subreddit": entry.subreddit,
        "kind": entry.kind,
        // NOTE: Reddit RSS carries no score; we deliberately omit it.
        "score": serde_json::Value::Null,
    });

    Some(RawMention {
        external_id: entry.external_id.clone(),
        content_text: entry.content_text.clone(),
        content_url: entry.content_url.clone(),
        author_name: entry.author.clone(),
        author_url,
        published_at: entry.published_at,
        platform_meta,
    })
}

/// One page's fetch result: parsed entries, the next `&after=` cursor (oldest
/// entry's id, or `None` at the end), the throttle [`Outcome`], and — on a
/// non-success outcome — a human-readable error detail for the target's sticky
/// `last_error` (e.g. `"request timed out"`, `"HTTP 503"`).
struct PageFetch {
    entries: Vec<ParsedEntry>,
    next_cursor: Option<String>,
    outcome: crate::ratelimit::Outcome,
    error: Option<String>,
}

/// Classify a transport-level `reqwest` error into a short, readable phrase so
/// `last_error` distinguishes the common Reddit failure modes (the CDN tarpits /
/// resets over-budget connections) instead of a generic "fetch failed".
fn describe_transport_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "request timed out".to_string()
    } else if e.is_connect() {
        "connection failed".to_string()
    } else {
        format!("request error: {e}")
    }
}

impl RedditCollector {
    /// Fetch exactly ONE page (no internal pacing/retry — the [`crate::collectors::scheduler`]
    /// runner owns pacing via the throttle). `cursor` is the `&after=` token for
    /// the next older page (or `None` for the head).
    async fn fetch_one_page(
        http: &reqwest::Client,
        url: &str,
        user_agent: &str,
        cursor: Option<&str>,
    ) -> PageFetch {
        let page_url = match cursor {
            Some(a) => format!("{}&after={}", url, a),
            None => url.to_string(),
        };
        let resp = match http
            .get(&page_url)
            .header("User-Agent", user_agent)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return PageFetch {
                    entries: Vec::new(),
                    next_cursor: None,
                    outcome: outcome_for_status(None, None),
                    error: Some(describe_transport_error(&e)),
                }
            }
        };
        let status = resp.status();
        let retry_after = parse_retry_after(
            resp.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
        );
        let outcome = outcome_for_status(Some(status.as_u16()), retry_after);
        if !status.is_success() {
            return PageFetch {
                entries: Vec::new(),
                next_cursor: None,
                outcome,
                error: Some(format!("HTTP {}", status.as_u16())),
            };
        }
        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) => {
                // Status was 2xx but the body transfer failed (truncated/reset
                // mid-stream — common on large responses to a throttled IP).
                return PageFetch {
                    entries: Vec::new(),
                    next_cursor: None,
                    outcome: outcome_for_status(None, None),
                    error: Some(format!("body read error: {}", describe_transport_error(&e))),
                };
            }
        };
        let entries = parse_feed(&body);
        // The oldest (last) entry's id is the cursor for the next older page.
        let next_cursor = entries.last().map(|e| e.external_id.clone());
        PageFetch {
            entries,
            next_cursor,
            outcome,
            error: None,
        }
    }
}

#[async_trait]
impl Collector for RedditCollector {
    fn channel(&self) -> &'static str {
        "reddit"
    }

    /// One-shot fetch for a single monitor, used by the `pulp query` exploration
    /// command (the production poll loop goes through the targeted
    /// [`scheduler`](crate::collectors::scheduler) runner via [`as_targeted`],
    /// not this). Pages the global search via [`fetch_one_page`] — the same
    /// single-page primitive the runner uses — with no shared throttle: it's a
    /// manual, one-off command, so on a 429/failure it simply stops and surfaces
    /// the error rather than sleeping through Reddit's penalty bucket.
    ///
    /// [`as_targeted`]: RedditCollector::as_targeted
    /// [`fetch_one_page`]: RedditCollector::fetch_one_page
    async fn fetch(
        &self,
        monitor: &Monitor,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        since: Option<i64>,
    ) -> anyhow::Result<Vec<RawMention>> {
        let settings: RedditSettings = serde_json::from_value(creds.clone()).unwrap_or_default();
        let ua = settings.user_agent();
        let base = Self::api_base();

        // Global search over this monitor's terms, chunked under the query cap.
        // Any `subreddits` scope is applied client-side by `entry_to_mention`.
        let queries = chunk_queries(monitor.terms.iter().map(String::as_str), MAX_QUERY_CHARS);
        let max_pages = max_backfill_pages();
        let mut results: Vec<RawMention> = Vec::new();
        // Dedups across pages AND chunks, and powers the "no new ids → stop"
        // condition (a source that ignores `after` repeats a page → nothing new).
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for q in &queries {
            let url = format!(
                "{}/search.rss?q={}&sort=new&limit=25",
                base,
                percent_encode(q)
            );
            let mut cursor: Option<String> = None;
            for page_index in 0..max_pages {
                let PageFetch {
                    entries,
                    next_cursor,
                    outcome,
                    error,
                } = Self::fetch_one_page(http, &url, ua, cursor.as_deref()).await;
                match outcome {
                    Outcome::Success => {}
                    Outcome::Throttled { .. } => {
                        if page_index == 0 {
                            anyhow::bail!("Reddit rate limited (HTTP 429) for {}", url);
                        }
                        break; // keep the partial result from earlier pages
                    }
                    Outcome::Failure => {
                        if page_index == 0 {
                            anyhow::bail!(
                                "Reddit fetch failed for {}: {}",
                                url,
                                error.as_deref().unwrap_or("unknown error")
                            );
                        }
                        break;
                    }
                }
                if entries.is_empty() {
                    break;
                }

                let oldest_ts_on_page = entries.iter().filter_map(|e| e.published_at).min();
                let mut new_on_page = 0usize;
                for entry in entries {
                    if !seen.insert(entry.external_id.clone()) {
                        continue;
                    }
                    new_on_page += 1;
                    if let Some(mention) = entry_to_mention(&entry, monitor, &settings, since) {
                        results.push(mention);
                    }
                }

                if !should_continue_paging(
                    new_on_page,
                    oldest_ts_on_page,
                    since,
                    page_index,
                    max_pages,
                ) {
                    break;
                }
                match next_cursor {
                    Some(a) => cursor = Some(a),
                    None => break,
                }
            }
        }

        Ok(results)
    }

    fn as_targeted(&self) -> Option<&dyn TargetedCollector> {
        Some(self)
    }
}

#[async_trait]
impl TargetedCollector for RedditCollector {
    /// Build the durable targets from the active monitors. Every monitor searches
    /// globally: ALL monitors' terms are OR-batched and chunked under the query
    /// cap into one search target per chunk. Each chunk's results are matched
    /// client-side against EVERY monitor (a term may land in any chunk, and
    /// subreddit scope is a client-side filter), so the members of every target
    /// are all monitors. Stashes them in `self.plan` for `fetch_target_page`.
    fn plan_targets(&self, inputs: &[MonitorFetch<'_>]) -> Vec<PlannedTarget> {
        let parsed: Vec<RedditSettings> = inputs
            .iter()
            .map(|i| serde_json::from_value(i.creds.clone()).unwrap_or_default())
            .collect();

        let base = Self::api_base();
        let channel = self.channel();
        let ua = parsed
            .first()
            .map(|s| s.user_agent().to_string())
            .unwrap_or_else(|| RedditSettings::default().user_agent().to_string());

        let queries = chunk_queries(
            flatten_terms(inputs.iter().map(|i| i.monitor)).into_iter(),
            MAX_QUERY_CHARS,
        );

        // Every chunk is matched against every monitor.
        let all_members: Vec<(Monitor, RedditSettings)> = inputs
            .iter()
            .enumerate()
            .map(|(i, mf)| (mf.monitor.clone(), parsed[i].clone()))
            .collect();
        let all_member_ids: Vec<String> = inputs.iter().map(|i| i.monitor.id.clone()).collect();

        let mut plan = self.plan.lock().unwrap();
        plan.clear();
        let mut targets: Vec<PlannedTarget> = Vec::new();
        for q in queries {
            let url = format!(
                "{}/search.rss?q={}&sort=new&limit=25",
                base,
                percent_encode(&q)
            );
            plan.insert(
                (TargetKind::Search.as_str().to_string(), q.clone()),
                all_members.clone(),
            );
            targets.push(PlannedTarget {
                kind: TargetKind::Search,
                descriptor: q,
                lane: channel.to_string(),
                member_monitor_ids: all_member_ids.clone(),
                request: TargetRequest {
                    url,
                    user_agent: ua.clone(),
                },
            });
        }

        targets
    }

    async fn fetch_target_page(
        &self,
        target: &PlannedTarget,
        http: &reqwest::Client,
        cursor: Option<&str>,
    ) -> TargetPage {
        let PageFetch {
            entries,
            next_cursor,
            outcome,
            error,
        } = Self::fetch_one_page(
            http,
            &target.request.url,
            &target.request.user_agent,
            cursor,
        )
        .await;

        let items: Vec<ParsedItem> = entries
            .iter()
            .map(|e| ParsedItem {
                external_id: e.external_id.clone(),
                published_at: e.published_at,
            })
            .collect();

        // Match the page's entries against each member monitor (client-side),
        // using the stashed (monitor, settings) from plan_targets. `since` is
        // None here: the scheduler owns the time-window via watermarks/jobs, and
        // applying a per-page `since` would drop the very backfill items a job is
        // fetching. Idempotency is preserved by dedup on external_id.
        let key = (target.kind.as_str().to_string(), target.descriptor.clone());
        let members = self
            .plan
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let mut mentions: Vec<(String, Vec<RawMention>)> = Vec::new();
        for (monitor, settings) in &members {
            let ms: Vec<RawMention> = entries
                .iter()
                .filter_map(|e| entry_to_mention(e, monitor, settings, None))
                .collect();
            mentions.push((monitor.id.clone(), ms));
        }

        TargetPage {
            items,
            next_cursor,
            outcome,
            error,
            mentions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(phrase: &str) -> Monitor {
        Monitor {
            id: "m1".into(),
            workspace_id: "w1".into(),
            terms: vec![phrase.into()],
            active: true,
            channels: vec!["reddit".into()],
            exact_match: false,
            case_sensitive: false,
            exclude_terms: vec![],
            channel_settings: serde_json::Value::Object(Default::default()),
            ai_filter_prompt: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>t3_abc123</id>
    <title>Anyone tried testbrand?</title>
    <link href="https://www.reddit.com/r/devops/comments/abc123/anyone_tried/"/>
    <author><name>/u/redditor1</name></author>
    <category term="devops" label="r/devops"/>
    <updated>2026-06-09T10:00:00+00:00</updated>
    <content type="html">&lt;p&gt;Looking for alternatives to testbrand&lt;/p&gt;</content>
  </entry>
  <entry>
    <id>tag:reddit.com,2005:t3_acc1</id>
    <title>testbrand screen reader support</title>
    <link href="https://www.reddit.com/r/accessibility/comments/acc1/x/"/>
    <author><name>/u/a11y_fan</name></author>
    <category term="accessibility" label="r/accessibility"/>
    <updated>2026-06-09T09:00:00+00:00</updated>
    <content type="html">solid accessibility work for testbrand</content>
  </entry>
</feed>"#;

    #[test]
    fn parses_reddit_atom() {
        let entries = parse_feed(SAMPLE);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].external_id, "t3_abc123");
        assert_eq!(entries[0].kind, "link");
        assert_eq!(entries[0].subreddit, "devops");
        assert_eq!(entries[0].author.as_deref(), Some("redditor1"));
        assert!(entries[0]
            .content_text
            .contains("alternatives to testbrand"));
        // tag-URI id is reduced to the thing-id.
        assert_eq!(entries[1].external_id, "t3_acc1");
        assert_eq!(entries[1].subreddit, "accessibility");
    }

    #[test]
    fn monitor_and_excludes_apply() {
        let entries = parse_feed(SAMPLE);
        let mut settings = RedditSettings::default();
        settings.exclude_subreddits = vec!["accessibility".into()];
        let kept: Vec<_> = entries
            .iter()
            .filter_map(|e| entry_to_mention(e, &monitor("testbrand"), &settings, None))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].external_id, "t3_abc123");
        // score is null, not invented.
        assert!(kept[0].platform_meta["score"].is_null());
    }

    #[test]
    fn blank_user_agent_falls_back_to_default() {
        // A `""` left in the channel credentials by the UI must not be sent as
        // the actual User-Agent — Reddit hard-throttles blank-UA clients.
        let blank: RedditSettings =
            serde_json::from_value(serde_json::json!({ "user_agent": "" })).unwrap();
        assert_eq!(blank.user_agent(), "pulp-social-listening/0.1");
        let spaces: RedditSettings =
            serde_json::from_value(serde_json::json!({ "user_agent": "   " })).unwrap();
        assert_eq!(spaces.user_agent(), "pulp-social-listening/0.1");
        let real: RedditSettings =
            serde_json::from_value(serde_json::json!({ "user_agent": "me:v1" })).unwrap();
        assert_eq!(real.user_agent(), "me:v1");
        assert_eq!(
            RedditSettings::default().user_agent(),
            "pulp-social-listening/0.1"
        );
    }

    #[test]
    fn time_window_drops_old() {
        let entries = parse_feed(SAMPLE);
        // since = far in the future -> everything dropped.
        let future = 9_000_000_000i64;
        let kept: Vec<_> = entries
            .iter()
            .filter_map(|e| {
                entry_to_mention(
                    e,
                    &monitor("testbrand"),
                    &RedditSettings::default(),
                    Some(future),
                )
            })
            .collect();
        assert!(kept.is_empty());
    }

    #[test]
    fn subreddit_scope_filters_client_side() {
        // `subreddits` is now an INCLUDE filter on global-search results, not its
        // own request. SAMPLE carries an r/devops and an r/accessibility entry.
        let entries = parse_feed(SAMPLE);
        let mut settings = RedditSettings::default();
        settings.subreddits = vec!["accessibility".into()];
        let scoped: Vec<_> = entries
            .iter()
            .filter_map(|e| entry_to_mention(e, &monitor("testbrand"), &settings, None))
            .collect();
        assert_eq!(scoped.len(), 1, "only the in-scope subreddit survives");
        // Empty scope = all subreddits.
        let all: Vec<_> = entries
            .iter()
            .filter_map(|e| {
                entry_to_mention(e, &monitor("testbrand"), &RedditSettings::default(), None)
            })
            .collect();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn chunk_queries_single_when_under_cap() {
        // Small term set → one OR-batched query (deduped + sorted, canonical).
        let qs = chunk_queries(["b", "a", "a", "c"].into_iter(), 512);
        assert_eq!(qs, vec![r#""a" OR "b" OR "c""#.to_string()]);
    }

    #[test]
    fn chunk_queries_splits_over_cap_and_covers_all_terms() {
        // A small cap forces multiple chunks; every term must appear in exactly
        // one chunk, and each chunk must stay within the cap.
        let terms = ["alpha", "bravo", "charlie", "delta", "echo"];
        let cap = 24;
        let qs = chunk_queries(terms.into_iter(), cap);
        assert!(qs.len() > 1, "expected multiple chunks, got {qs:?}");
        for q in &qs {
            assert!(q.len() <= cap, "chunk over cap ({} chars): {q:?}", q.len());
        }
        for t in terms {
            let hits = qs
                .iter()
                .filter(|q| q.contains(&format!("\"{t}\"")))
                .count();
            assert_eq!(hits, 1, "term {t} should appear in exactly one chunk");
        }
    }

    #[test]
    fn chunk_queries_keeps_an_oversized_term_in_its_own_chunk() {
        // A single term longer than the cap isn't dropped — it gets its own chunk.
        let long = "a_very_long_single_search_term_exceeding_the_cap";
        let qs = chunk_queries([long, "x"].into_iter(), 16);
        assert!(
            qs.iter().any(|q| q.contains(long)),
            "long term retained: {qs:?}"
        );
        assert!(qs.iter().any(|q| q == r#""x""#));
    }

    #[test]
    fn chunk_queries_is_order_independent() {
        assert_eq!(
            chunk_queries(["c", "a", "b"].into_iter(), 512),
            chunk_queries(["a", "b", "c"].into_iter(), 512),
        );
    }

    #[test]
    fn or_query_quotes_terms_and_joins_with_or() {
        let q = global_or_query(["desktop automation", "pywinauto"].into_iter());
        assert_eq!(q, r#""desktop automation" OR "pywinauto""#);
        // A single term is just quoted (exact-phrase search).
        assert_eq!(global_or_query(["nimbusdb"].into_iter()), r#""nimbusdb""#);
        // And the encoded form stays a valid query value.
        assert_eq!(percent_encode(r#""a b" OR "c""#), "%22a+b%22+OR+%22c%22");
    }

    #[test]
    fn or_query_strips_embedded_quotes() {
        // An unescaped `"` inside a term would otherwise prematurely close
        // its quoted phrase and corrupt the rest of the OR-batched query for
        // every monitor sharing it.
        let q = global_or_query([r#"Nimbus "Pro" plan"#, "fernlint"].into_iter());
        assert_eq!(q, r#""Nimbus Pro plan" OR "fernlint""#);
    }

    #[test]
    fn or_query_dedups_and_skips_blanks() {
        // Blank/whitespace terms are dropped; duplicates collapse.
        let q = global_or_query(["a", "", "  ", "b", "a"].into_iter());
        assert_eq!(q, r#""a" OR "b""#);
    }

    #[test]
    fn flatten_terms_across_monitors_then_or_batch() {
        // Two global-search monitors, multi-term each, all OR-ed into one query.
        let mut m1 = monitor("FlaUI");
        m1.terms = vec!["FlaUI".into(), "Ranorex".into()];
        let mut m2 = monitor("WinAppDriver");
        m2.terms = vec!["WinAppDriver".into()];
        let flat = flatten_terms([&m1, &m2].into_iter());
        assert_eq!(flat, vec!["FlaUI", "Ranorex", "WinAppDriver"]);
        let q = global_or_query(flat.into_iter());
        assert_eq!(q, r#""FlaUI" OR "Ranorex" OR "WinAppDriver""#);
    }

    #[test]
    fn or_query_is_order_independent() {
        // Same term set in any order → identical query, so reordering a monitor's
        // terms doesn't re-key its target (canonical descriptor).
        let a = global_or_query(["b", "a", "c"].into_iter());
        let b = global_or_query(["c", "a", "b"].into_iter());
        assert_eq!(a, b);
        assert_eq!(a, r#""a" OR "b" OR "c""#);
    }

    #[test]
    fn plan_targets_global_search_chunked_on_one_lane() {
        // Two monitors (one subreddit-scoped) → their terms OR-batched into a
        // SINGLE global-search target (fits one chunk), on the shared "reddit"
        // lane. The subreddit scope is client-side, not its own request: no
        // new.rss firehose, no restrict_sr per-sub search.
        let m_a = monitor("alpha");
        let mut m_b = monitor("beta");
        m_b.terms = vec!["beta".into(), "gamma".into()];
        let inputs = vec![
            MonitorFetch {
                monitor: &m_a,
                creds: serde_json::json!({}),
            },
            MonitorFetch {
                monitor: &m_b,
                creds: serde_json::json!({ "subreddits": ["rust", "golang"] }),
            },
        ];
        let targets = RedditCollector::new().plan_targets(&inputs);
        assert_eq!(targets.len(), 1, "all terms batched into one global search");
        let t = &targets[0];
        assert_eq!(t.lane, "reddit");
        assert_eq!(t.kind, TargetKind::Search);
        assert!(t.request.url.contains("/search.rss?q="));
        assert!(!t.request.url.contains("new.rss"));
        assert!(!t.request.url.contains("restrict_sr"));
        assert_eq!(t.descriptor, r#""alpha" OR "beta" OR "gamma""#);
        // Each chunk is matched against every monitor, so all are members.
        assert_eq!(t.member_monitor_ids.len(), 2);
    }

    #[test]
    fn plan_targets_chunks_when_query_exceeds_cap() {
        // A long term list splits into multiple global-search targets, all on the
        // one shared lane and all hitting /search.rss.
        let mut m = monitor("x");
        m.terms = (0..50)
            .map(|i| format!("longsearchterm_number_{i:02}"))
            .collect();
        let inputs = vec![MonitorFetch {
            monitor: &m,
            creds: serde_json::json!({}),
        }];
        let targets = RedditCollector::new().plan_targets(&inputs);
        assert!(targets.len() > 1, "oversized term list splits into chunks");
        assert!(targets.iter().all(|t| t.lane == "reddit"));
        assert!(targets
            .iter()
            .all(|t| t.request.url.contains("/search.rss?q=")));
    }
}
