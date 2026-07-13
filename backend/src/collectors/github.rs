use async_trait::async_trait;
use serde::Deserialize;

use super::github_filter::{is_ignored, GitHubSettings};
use crate::collectors::{
    matches_monitor, max_backfill_pages, percent_encode, should_continue_paging, Collector,
    RawMention,
};
use crate::db::repos::traits::Monitor;

pub struct GitHubCollector;

impl GitHubCollector {
    fn base_url() -> String {
        std::env::var("GITHUB_BASE_URL").unwrap_or_else(|_| "https://api.github.com".to_string())
    }
}

#[derive(Deserialize)]
struct GithubSearchResponse {
    items: Vec<GithubIssue>,
}

#[derive(Deserialize)]
struct GithubIssue {
    id: i64,
    title: String,
    body: Option<String>,
    html_url: String,
    user: Option<GithubUser>,
    created_at: Option<String>,
    state: Option<String>,
    repository_url: Option<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    login: String,
    html_url: String,
}

#[async_trait]
impl Collector for GitHubCollector {
    fn channel(&self) -> &'static str {
        "github"
    }

    async fn fetch(
        &self,
        monitor: &Monitor,
        http: &reqwest::Client,
        creds: &serde_json::Value,
        since: Option<i64>,
    ) -> anyhow::Result<Vec<RawMention>> {
        let token = creds.get("token").and_then(|v| v.as_str()).unwrap_or("");
        let settings: GitHubSettings = serde_json::from_value(creds.clone()).unwrap_or_default();

        // Flatten the monitor's terms into one GitHub search query, each term
        // quoted and OR-joined (matches the client-side `matches_monitor`
        // re-filter below). GitHub code/issue search treats a quoted string as
        // an exact phrase and supports `OR` between them.
        let query = github_terms_query(&monitor.terms);
        let mut q = percent_encode(&query);
        match settings.state_filter.as_str() {
            "open" => q.push_str("+is:open"),
            "closed" => q.push_str("+is:closed"),
            _ => {} // "all" — no filter appended
        }
        // The search API has no working `since` param; the correct way to floor
        // results by creation date is the `created:>=<date>` qualifier (we ALSO
        // filter client-side on `created_at` below, so this is best-effort).
        if let Some(ts) = since {
            let dt = chrono::DateTime::from_timestamp(ts, 0)
                .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
            q.push_str(&format!("+created:%3E%3D{}", dt.format("%Y-%m-%d")));
        }

        // GitHub search caps at 1000 results = 10 pages of 100; we additionally
        // bound by the shared backfill cap.
        let base = format!(
            "{}/search/issues?q={}&sort=created&order=desc&per_page=100",
            Self::base_url(),
            q
        );

        let max_pages = max_backfill_pages();
        let mut results = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut page: u32 = 1; // GitHub search pages are 1-indexed.
        let mut page_index: u32 = 0; // 0-based for the stop helper / cap.

        loop {
            let url = format!("{}&page={}", base, page);

            let mut req = http
                .get(&url)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header("User-Agent", "Pulp/0.1");

            if !token.is_empty() {
                req = req.bearer_auth(token);
            }

            let resp = req.send().await?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                // Marker error: [`Collector::fetch_pass`]'s default impl
                // detects this via downcast and aborts the rest of the pass
                // instead of hammering an already-throttled API.
                return Err(crate::collectors::RateLimited { url: url.clone() }.into());
            }
            if !resp.status().is_success() {
                anyhow::bail!("GitHub API returned status {}", resp.status());
            }

            let data: GithubSearchResponse = resp.json().await?;
            if data.items.is_empty() {
                break;
            }

            let mut new_on_page = 0usize;
            let mut oldest_ts_on_page: Option<i64> = None;

            for issue in data.items {
                let external_id = format!("gh_{}", issue.id);

                let published_at = issue
                    .created_at
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp());
                if let Some(ts) = published_at {
                    oldest_ts_on_page = Some(oldest_ts_on_page.map_or(ts, |o| o.min(ts)));
                }

                if !seen.insert(external_id.clone()) {
                    continue;
                }
                new_on_page += 1;

                // Client-side `since` floor (the search API can't reliably do it).
                if let (Some(since_ts), Some(pub_ts)) = (since, published_at) {
                    if pub_ts < since_ts {
                        continue;
                    }
                }

                let body_excerpt = issue
                    .body
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(500)
                    .collect::<String>();

                let content_text = if body_excerpt.is_empty() {
                    issue.title.clone()
                } else {
                    format!("{} {}", issue.title, body_excerpt)
                };

                if !matches_monitor(monitor, &content_text) {
                    continue;
                }

                let author_name = issue.user.as_ref().map(|u| u.login.clone());
                let author_url = issue.user.as_ref().map(|u| u.html_url.clone());

                // Extract repo full name from repository_url
                let repo = issue
                    .repository_url
                    .as_deref()
                    .and_then(|url| url.strip_prefix("https://api.github.com/repos/"))
                    .unwrap_or("")
                    .to_string();

                let author_for_filter = author_name.as_deref().unwrap_or("");
                if is_ignored(&settings, &repo, author_for_filter) {
                    continue;
                }

                let platform_meta = serde_json::json!({
                    "repo": repo,
                    "state": issue.state,
                    "type": "issue",
                });

                results.push(RawMention {
                    external_id,
                    content_text,
                    content_url: issue.html_url,
                    author_name,
                    author_url,
                    published_at,
                    platform_meta,
                });
            }

            if !should_continue_paging(new_on_page, oldest_ts_on_page, since, page_index, max_pages)
            {
                break;
            }
            page += 1;
            page_index += 1;
        }

        Ok(results)
    }
}

/// Build the GitHub search query for a monitor's terms: each term quoted (exact
/// phrase) and OR-joined. Empty/whitespace terms and duplicates are dropped. An
/// empty list yields an empty query (the upstream then returns nothing useful,
/// and the client-side match drops everything anyway).
fn github_terms_query(terms: &[String]) -> String {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    terms
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .filter(|t| seen.insert(*t))
        // Strip embedded double quotes before wrapping in a quoted phrase: an
        // unescaped `"` would otherwise prematurely close the phrase and
        // corrupt the rest of the OR-joined query.
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_terms_query_quotes_and_ors_terms() {
        let terms = vec!["nimbusdb".to_string(), "fernlint".to_string()];
        assert_eq!(github_terms_query(&terms), r#""nimbusdb" OR "fernlint""#);
    }

    #[test]
    fn github_terms_query_dedups_trims_and_drops_blanks() {
        let terms = vec![
            "  nimbusdb  ".to_string(),
            "".to_string(),
            "nimbusdb".to_string(),
        ];
        assert_eq!(github_terms_query(&terms), r#""nimbusdb""#);
    }

    #[test]
    fn github_terms_query_strips_embedded_quotes() {
        // An unescaped `"` inside a term would otherwise prematurely close
        // its quoted phrase and corrupt the rest of the OR-joined query.
        let terms = vec![r#"Nimbus "Pro" plan"#.to_string(), "fernlint".to_string()];
        assert_eq!(
            github_terms_query(&terms),
            r#""Nimbus Pro plan" OR "fernlint""#
        );
    }
}
