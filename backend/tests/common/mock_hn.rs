use axum::{extract::Query, extract::State, routing::get, Json, Router};
use chrono::Utc;
use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

pub async fn spawn() -> String {
    spawn_counted().await.0
}

/// Observations about the `/api/v1/search_by_date` requests the mock
/// received, so tests can assert request *volume* and the exact `query` sent
/// on each one — e.g. that a multi-term monitor issues one request PER term
/// (true OR) rather than one joined-query request (which Algolia would treat
/// as AND-of-all-words).
#[derive(Default)]
pub struct SearchSpy {
    pub hits: AtomicUsize,
    pub queries: Mutex<Vec<String>>,
}

/// Like [`spawn`], also returning a [`SearchSpy`] recording every request.
pub async fn spawn_counted() -> (String, Arc<SearchSpy>) {
    let spy = Arc::new(SearchSpy::default());
    let app = Router::new()
        .route("/api/v1/search_by_date", get(handler))
        .with_state(spy.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app).into_future());
    (format!("http://{}", addr), spy)
}

/// A single fixture hit: object id, tags, age-in-days, and a flag for whether it
/// is a comment (vs a story). Text echoes the query so it survives the monitor
/// pre-filter.
struct Hit {
    id: &'static str,
    is_comment: bool,
    age_days: i64,
}

/// Two pages of hits, page-2 strictly OLDER than page-1. The very last hit
/// (`333333`, 10 days old) sits beyond the 3-day backfill `since` cutoff so a
/// recent-`since` backfill never reaches it (server-side `numericFilters` drops
/// it), while a far-back backfill collects it from page 2.
const PAGES: [&[Hit]; 2] = [
    &[
        Hit {
            id: "111111",
            is_comment: true,
            age_days: 1,
        },
        Hit {
            id: "222222",
            is_comment: false,
            age_days: 2,
        },
    ],
    &[
        // 4 days old — within a 7-day backfill, page 2 only.
        Hit {
            id: "444444",
            is_comment: true,
            age_days: 4,
        },
        // 10 days old — beyond the 3-day cutoff used by the filter test.
        Hit {
            id: "333333",
            is_comment: false,
            age_days: 10,
        },
    ],
];

async fn handler(
    State(spy): State<Arc<SearchSpy>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let query = params.get("query").cloned().unwrap_or_default();
    spy.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    spy.queries.lock().unwrap().push(query.clone());

    // Mirror real Algolia semantics: comma-joined tags are ANDed, so
    // `tags=comment,story` matches nothing — only the OR form `(comment,story)`
    // returns hits. Guards against regressing to the AND form.
    if params.get("tags").map(String::as_str) != Some("(comment,story)") {
        return Json(serde_json::json!({
            "hits": [], "nbHits": 0, "page": 0, "nbPages": 0
        }));
    }

    // Parse the numericFilters param to get a since threshold.
    // Format: "created_at_i>TIMESTAMP"
    let since_threshold: Option<i64> = params.get("numericFilters").and_then(|f| {
        // f looks like "created_at_i>1234567890"
        f.strip_prefix("created_at_i>")
            .and_then(|ts| ts.parse::<i64>().ok())
    });

    // Algolia pages are 0-indexed via `&page=`. Out-of-range pages return empty.
    let page: usize = params.get("page").and_then(|p| p.parse().ok()).unwrap_or(0);

    let now = Utc::now().timestamp();

    let page_hits: &[Hit] = PAGES.get(page).copied().unwrap_or(&[]);

    let hits: Vec<serde_json::Value> = page_hits
        .iter()
        .filter_map(|h| {
            let ts = now - h.age_days * 86400;
            // Server-side `since` filter via numericFilters.
            if let Some(threshold) = since_threshold {
                if ts <= threshold {
                    return None;
                }
            }
            let created_at = chrono::DateTime::from_timestamp(ts, 0)
                .unwrap()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            Some(if h.is_comment {
                serde_json::json!({
                    "objectID": h.id,
                    "_tags": ["comment"],
                    // HTML tags + entities, as HN's API actually serves them —
                    // the collector must strip these at ingest.
                    "comment_text": format!("<p>I've been using {} &amp; it's great for monitoring</p>", query),
                    "story_title": format!("Ask HN: best tools like {}?", query),
                    "story_url": format!("https://example.com/article/{}", h.id),
                    "author": "testuser",
                    "created_at": created_at,
                    "created_at_i": ts,
                    "points": null,
                    "num_comments": null
                })
            } else {
                serde_json::json!({
                    "objectID": h.id,
                    "_tags": ["story"],
                    "title": format!("Show HN: New tool similar to {}", query),
                    "story_text": format!("We built this after using {} for a year", query),
                    "url": format!("https://example.com/tool/{}", h.id),
                    "author": "founder",
                    "created_at": created_at,
                    "created_at_i": ts,
                    "points": 42,
                    "num_comments": 15
                })
            })
        })
        .collect();

    let nb = hits.len();
    Json(serde_json::json!({
        "hits": hits,
        "nbHits": nb,
        "page": page,
        "nbPages": PAGES.len(),
    }))
}
