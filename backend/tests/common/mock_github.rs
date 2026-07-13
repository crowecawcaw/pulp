use axum::{extract::Query, routing::get, Json, Router};
use chrono::Utc;
use std::collections::HashMap;
use std::future::IntoFuture;
use tokio::net::TcpListener;

pub async fn spawn() -> String {
    let app = Router::new().route("/search/issues", get(handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app).into_future());
    format!("http://{}", addr)
}

/// A single fixture issue: numeric id and age-in-days. Text echoes the query so
/// it survives the monitor pre-filter.
struct Issue {
    id: i64,
    age_days: i64,
}

/// Two pages of issues, page-2 strictly OLDER than page-1. The GitHub search API
/// has no working `since` param, so the collector floors by `created_at`
/// client-side; the mock serves all pages and lets the collector drop old items.
/// The last issue (10 days old) sits beyond the 3-day backfill cutoff used by the
/// filter test, so a recent-`since` backfill collects only page-1 items while a
/// far-back backfill reaches it on page 2.
const PAGES: [&[Issue]; 2] = [
    &[
        Issue {
            id: 9999999,
            age_days: 1,
        },
        Issue {
            id: 7777777,
            age_days: 2,
        },
    ],
    &[
        Issue {
            id: 6666666,
            age_days: 4,
        },
        Issue {
            id: 8888888,
            age_days: 10,
        },
    ],
];

async fn handler(Query(params): Query<HashMap<String, String>>) -> Json<serde_json::Value> {
    let q = params.get("q").cloned().unwrap_or_default();

    // GitHub search pages are 1-indexed via `&page=`. Out-of-range → empty.
    let page: usize = params.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
    let idx = page.saturating_sub(1);

    let now = Utc::now().timestamp();
    let page_issues: &[Issue] = PAGES.get(idx).copied().unwrap_or(&[]);

    let items: Vec<serde_json::Value> = page_issues
        .iter()
        .map(|iss| {
            let ts = now - iss.age_days * 86400;
            let created_at = chrono::DateTime::from_timestamp(ts, 0)
                .unwrap()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            serde_json::json!({
                "id": iss.id,
                "number": iss.id,
                "title": format!("Feature request: integrate with {}", q),
                "body": format!("We use {} and would love better integration", q),
                "html_url": format!("https://github.com/example/repo/issues/{}", iss.id),
                "user": { "login": "developer1", "html_url": "https://github.com/developer1" },
                "created_at": created_at,
                "state": "open",
                "repository_url": "https://api.github.com/repos/example/repo"
            })
        })
        .collect();

    let total = items.len();
    Json(serde_json::json!({
        "total_count": total,
        "items": items
    }))
}
