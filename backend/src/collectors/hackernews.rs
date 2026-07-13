use async_trait::async_trait;
use serde::Deserialize;

use crate::collectors::rss_parse::strip_html;
use crate::collectors::{
    matches_monitor, max_backfill_pages, percent_encode, should_continue_paging, Collector,
    RawMention,
};
use crate::db::repos::traits::Monitor;

pub struct HackerNewsCollector;

impl HackerNewsCollector {
    fn base_url() -> String {
        std::env::var("HACKERNEWS_BASE_URL")
            .unwrap_or_else(|_| "https://hn.algolia.com".to_string())
    }
}

#[derive(Deserialize)]
struct HnResponse {
    hits: Vec<HnHit>,
}

#[derive(Deserialize)]
struct HnHit {
    #[serde(rename = "objectID")]
    object_id: String,
    #[serde(rename = "_tags")]
    tags: Option<Vec<String>>,
    comment_text: Option<String>,
    title: Option<String>,
    story_text: Option<String>,
    url: Option<String>,
    /// Comment hits carry the parent story's title/url (the article the
    /// discussion is about); story hits leave these unset.
    story_title: Option<String>,
    story_url: Option<String>,
    author: Option<String>,
    created_at: Option<String>,
    points: Option<i64>,
    num_comments: Option<i64>,
}

#[async_trait]
impl Collector for HackerNewsCollector {
    fn channel(&self) -> &'static str {
        "hackernews"
    }

    async fn fetch(
        &self,
        monitor: &Monitor,
        http: &reqwest::Client,
        _creds: &serde_json::Value,
        since: Option<i64>,
    ) -> anyhow::Result<Vec<RawMention>> {
        // Algolia tag syntax: comma = AND, parentheses = OR. `comment,story`
        // (comment AND story) matches nothing — the OR form is required.
        //
        // Algolia's `query` full-text search has NO boolean OR operator —
        // it defaults to requiring every word (AND), and `advancedSyntax`
        // only adds phrase-quoting and `-exclusion`, not OR (confirmed:
        // algolia/hn-search#169 is an open request for OR support). A
        // monitor matches on ANY of its terms, so space-joining them into one
        // query — as this used to do — under-recalls: only hits containing
        // EVERY term would ever surface. Instead issue one search per term
        // and union the (deduped) results, which is a true OR at the cost of
        // one request per term. The client-side `matches_monitor` below still
        // enforces the precise match semantics on whatever comes back.
        let max_pages = max_backfill_pages();
        let mut results = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for term in distinct_terms(&monitor.terms) {
            let base = format!(
                "{}/api/v1/search_by_date?query={}&tags=(comment,story)&hitsPerPage=50",
                Self::base_url(),
                percent_encode(term)
            );
            let mut page: u32 = 0;

            loop {
                // Algolia pages are 0-indexed via `&page=`. `created_at_i>` floors the
                // results server-side, but we still stop client-side on the cap and on
                // the oldest hit dropping below `since`.
                let mut url = format!("{}&page={}", base, page);
                if let Some(ts) = since {
                    url.push_str(&format!("&numericFilters=created_at_i>{}", ts));
                }

                let resp = http.get(&url).send().await?;
                if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    // Marker error: [`Collector::fetch_pass`]'s default impl
                    // detects this via downcast and aborts the rest of the pass
                    // instead of hammering an already-throttled API.
                    return Err(crate::collectors::RateLimited { url: url.clone() }.into());
                }
                if !resp.status().is_success() {
                    anyhow::bail!("HN API returned status {}", resp.status());
                }

                let data: HnResponse = resp.json().await?;
                if data.hits.is_empty() {
                    break;
                }

                let mut new_on_page = 0usize;
                let mut oldest_ts_on_page: Option<i64> = None;

                for hit in data.hits {
                    let published_at = hit
                        .created_at
                        .as_deref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.timestamp());
                    if let Some(ts) = published_at {
                        oldest_ts_on_page = Some(oldest_ts_on_page.map_or(ts, |o| o.min(ts)));
                    }

                    if !seen.insert(hit.object_id.clone()) {
                        // Already collected (source ignored the cursor / overlap) —
                        // not a NEW item, so it shouldn't keep the loop alive.
                        continue;
                    }
                    new_on_page += 1;

                    let is_comment = hit
                        .tags
                        .as_ref()
                        .map(|t| t.contains(&"comment".to_string()))
                        .unwrap_or(false);

                    // HN serves HTML (tags + entities) in comment/story bodies; strip
                    // it at ingest like the Reddit collector so the feed shows clean
                    // text. `title` is the article/story title — for a comment, the
                    // parent story's title (so the UI can show what it's about).
                    //
                    // For stories, content_text keeps the title prepended (as before)
                    // so phrase matching and alert criteria still see it; the UI and
                    // CLI de-duplicate against platform_meta.title. Comments match on
                    // their own text — the parent title is context, not theirs.
                    let (kind, title, content_text) = if is_comment {
                        (
                            "comment",
                            strip_html(hit.story_title.as_deref().unwrap_or_default()),
                            strip_html(hit.comment_text.as_deref().unwrap_or_default()),
                        )
                    } else {
                        let title = strip_html(hit.title.as_deref().unwrap_or_default());
                        let story = strip_html(hit.story_text.as_deref().unwrap_or_default());
                        let content_text = if story.trim().is_empty() {
                            title.clone()
                        } else {
                            format!("{} {}", title, story)
                        };
                        ("story", title, content_text)
                    };

                    if content_text.trim().is_empty() {
                        continue;
                    }

                    if !matches_monitor(monitor, &content_text) {
                        continue;
                    }

                    // Link to the HN discussion (where the conversation we matched
                    // actually lives), not the story's external article. The article
                    // URL — if any — is kept in platform_meta so the UI can still
                    // offer an "open article" link.
                    let content_url =
                        format!("https://news.ycombinator.com/item?id={}", hit.object_id);
                    let article_url = if is_comment { &hit.story_url } else { &hit.url };

                    let platform_meta = serde_json::json!({
                        "points": hit.points,
                        "num_comments": hit.num_comments,
                        "kind": kind,
                        "title": (!title.trim().is_empty()).then_some(title),
                        "story_url": article_url,
                    });

                    results.push(RawMention {
                        external_id: hit.object_id,
                        content_text,
                        content_url,
                        author_name: hit.author,
                        author_url: None,
                        published_at,
                        platform_meta,
                    });
                }

                if !should_continue_paging(new_on_page, oldest_ts_on_page, since, page, max_pages) {
                    break;
                }
                page += 1;
            }
        }

        Ok(results)
    }
}

/// Trim, drop blanks, and de-duplicate a monitor's terms (order-preserving)
/// — the per-term list [`Collector::fetch`] issues one Algolia search for.
fn distinct_terms(terms: &[String]) -> Vec<&str> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    terms
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .filter(|t| seen.insert(*t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_terms_trims_dedups_and_drops_blanks() {
        let terms = vec![
            "  nimbusdb  ".to_string(),
            "".to_string(),
            "fernlint".to_string(),
            "nimbusdb".to_string(),
            "   ".to_string(),
        ];
        assert_eq!(distinct_terms(&terms), vec!["nimbusdb", "fernlint"]);
    }

    #[test]
    fn distinct_terms_one_request_per_distinct_term() {
        // The regression this guards: a multi-term monitor used to be joined
        // into a single space-separated query, which Algolia treats as
        // AND-of-all-words (no OR operator exists), so only hits containing
        // every term would ever surface. `fetch` now issues one search per
        // entry in this list, which is the actual OR-recall fix.
        let terms = vec!["nimbusdb".to_string(), "fernlint".to_string()];
        assert_eq!(distinct_terms(&terms).len(), 2);
    }
}
