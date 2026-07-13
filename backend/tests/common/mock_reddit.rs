use axum::{
    extract::{Path, Query, State},
    http::header,
    response::IntoResponse,
    routing::get,
    Router,
};
use chrono::{SecondsFormat, Utc};
use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

/// Mock of Reddit's **public, unauthenticated RSS/Atom** endpoints.
///
/// The real collector uses `*.rss` (Reddit's JSON API 403s unauthenticated), so
/// this mock serves Atom XML. We expose the three endpoints the collector can
/// hit: global `search.rss`, per-sub `search.rss`, and the `new.rss` firehose.
/// All three return the same varied dataset.
///
/// The dataset is deliberately varied by **subreddit / author / text** (NOT
/// score — RSS has no score), plus one entry (`t3_oldpost`) with an `<updated>`
/// timestamp ~10 days in the past so the backfill time-window test has something
/// to exclude.
///
/// ## Pagination
/// The mock honors Reddit's `&after=<fullname>` cursor: page 1 (no `after`)
/// serves the recent dataset; passing `after=t3_oldpost` (the last id of page 1)
/// serves page 2, whose single entry (`t3_page2deep`) is ~12 days old — OLDER
/// than everything on page 1 and beyond the 3-day backfill cutoff. Any other
/// `after` value returns an empty feed (end of results), so the collector's
/// stop-conditions terminate.
pub async fn spawn() -> String {
    spawn_counted().await.0
}

/// Observations about the global `search.rss` requests the mock received, so
/// tests can assert request *volume* (e.g. OR-batching collapses N monitors
/// into one search) and the exact query sent.
///
/// `throttle_first_n` (when > 0) makes the global `search.rss` route return
/// HTTP 429 (with a short `Retry-After`) for that many initial requests, then
/// serve normally — to exercise the auto-recovering runner's throttle + resume.
#[derive(Default)]
pub struct SearchSpy {
    pub hits: AtomicUsize,
    pub last_q: Mutex<String>,
    pub throttle_first_n: AtomicUsize,
    pub throttled_count: AtomicUsize,
    /// When > 0, the first N global-search requests return HTTP 503 (a non-429
    /// failure) so tests can assert the real status is surfaced into last_error.
    pub fail_first_n: AtomicUsize,
    pub failed_count: AtomicUsize,
}

/// Like [`spawn`], also returning a [`SearchSpy`] for the global search route.
pub async fn spawn_counted() -> (String, Arc<SearchSpy>) {
    spawn_with_spy(Arc::new(SearchSpy::default())).await
}

/// Like [`spawn_counted`] but the global `search.rss` route returns HTTP 429 for
/// the first `throttle_first_n` requests (with `Retry-After: 1`) before serving
/// the normal dataset. Use to drive the throttle/backoff/recovery path.
pub async fn spawn_throttling(throttle_first_n: usize) -> (String, Arc<SearchSpy>) {
    let spy = Arc::new(SearchSpy::default());
    spy.throttle_first_n
        .store(throttle_first_n, std::sync::atomic::Ordering::SeqCst);
    spawn_with_spy(spy).await
}

/// Like [`spawn_counted`] but the global `search.rss` route returns HTTP 503 for
/// the first `fail_first_n` requests (a non-429 failure) before serving normally.
/// Use to assert the runner records the real status (`HTTP 503`) into last_error
/// rather than a generic "fetch failed".
pub async fn spawn_failing(fail_first_n: usize) -> (String, Arc<SearchSpy>) {
    let spy = Arc::new(SearchSpy::default());
    spy.fail_first_n
        .store(fail_first_n, std::sync::atomic::Ordering::SeqCst);
    spawn_with_spy(spy).await
}

async fn spawn_with_spy(spy: Arc<SearchSpy>) -> (String, Arc<SearchSpy>) {
    let app = Router::new()
        .route("/search.rss", get(search_handler))
        .route("/r/:sub/search.rss", get(sub_search_handler))
        .route("/r/:sub/new.rss", get(new_handler))
        .with_state(spy.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app).into_future());
    (format!("http://{}", addr), spy)
}

async fn search_handler(
    State(spy): State<Arc<SearchSpy>>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    use std::sync::atomic::Ordering::SeqCst;
    let q = params.get("q").cloned().unwrap_or_default();
    spy.hits.fetch_add(1, SeqCst);
    *spy.last_q.lock().unwrap() = q.clone();

    // Throttle the first N requests with a 429 + short Retry-After.
    let already_throttled = spy.throttled_count.load(SeqCst);
    if already_throttled < spy.throttle_first_n.load(SeqCst) {
        spy.throttled_count.fetch_add(1, SeqCst);
        return (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "1")],
            "rate limited",
        )
            .into_response();
    }

    // Fail the first N requests with a non-429 status (503).
    let already_failed = spy.failed_count.load(SeqCst);
    if already_failed < spy.fail_first_n.load(SeqCst) {
        spy.failed_count.fetch_add(1, SeqCst);
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "upstream unavailable",
        )
            .into_response();
    }

    atom_response(&paged_feed(&q, params.get("after").map(String::as_str))).into_response()
}

async fn sub_search_handler(
    Path(_sub): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let q = params.get("q").cloned().unwrap_or_default();
    atom_response(&paged_feed(&q, params.get("after").map(String::as_str)))
}

async fn new_handler(
    Path(_sub): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Firehose has no query; the collector matches monitors client-side. The
    // fixture text already embeds the test phrase so it survives that filter.
    atom_response(&paged_feed(
        "testbrand",
        params.get("after").map(String::as_str),
    ))
}

/// Cursor-aware feed dispatch. `after == None` → page 1; `after == "t3_oldpost"`
/// (page 1's last id) → page 2; anything else → empty (end of results).
fn paged_feed(q: &str, after: Option<&str>) -> String {
    match after {
        None => feed(q),
        Some("t3_oldpost") => page_two(q),
        Some(_) => empty_feed(),
    }
}

fn atom_response(body: &str) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/atom+xml; charset=UTF-8")],
        body.to_string(),
    )
}

/// Build an Atom feed whose entries echo the query `q` so they survive the
/// collector's monitor pre-filter, then differ by subreddit / author / text so
/// criteria can discriminate without relying on score.
fn feed(q: &str) -> String {
    let now = Utc::now();
    let recent = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    let old = (now - chrono::Duration::days(10)).to_rfc3339_opts(SecondsFormat::Secs, true);

    let entries = [
        // Accessibility subreddit, clean.
        entry(
            "t3_acc1",
            &format!("{} screen reader support is great", q),
            &format!("solid accessibility work for {}", q),
            "accessibility",
            "a11y_fan",
            "acc1",
            &recent,
        ),
        // devops, clean, distinct author.
        entry(
            "t3_low1",
            &format!("quick {} question", q),
            &format!("minor thing about {}", q),
            "devops",
            "dev1",
            "low1",
            &recent,
        ),
        // Spammy text + spammer author in the deals subreddit.
        entry(
            "t3_spam1",
            &format!("buy {} cheap spam discount", q),
            &format!("spam spam {} sale", q),
            "deals",
            "spammer",
            "spam1",
            &recent,
        ),
        // The original fixture, devops, clean.
        entry(
            "t3_abc123",
            &format!("Anyone tried {}?", q),
            &format!("Looking for alternatives to {}", q),
            "devops",
            "redditor1",
            "abc123",
            &recent,
        ),
        // Outside the backfill window — dropped client-side by the collector.
        entry(
            "t3_oldpost",
            &format!("Old discussion about {}", q),
            &format!("This is old content mentioning {}", q),
            "devops",
            "redditor2",
            "oldpost",
            &old,
        ),
    ]
    .join("\n");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>mock reddit</title>
{}
</feed>"#,
        entries
    )
}

/// Page 2 of the feed (served when `after=t3_oldpost`). A single entry that is
/// OLDER than everything on page 1 (~12 days) and beyond the 3-day backfill
/// cutoff — only a far-back backfill reaches it.
fn page_two(q: &str) -> String {
    let now = Utc::now();
    let deep = (now - chrono::Duration::days(12)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let entry = entry(
        "t3_page2deep",
        &format!("Even older {} thread", q),
        &format!("page two content about {}", q),
        "devops",
        "redditor3",
        "page2deep",
        &deep,
    );
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>mock reddit page 2</title>
{}
</feed>"#,
        entry
    )
}

/// An empty feed (end of results) — the collector stops when a page yields no
/// new items.
fn empty_feed() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>mock reddit empty</title>
</feed>"#
        .to_string()
}

#[allow(clippy::too_many_arguments)]
fn entry(
    thing_id: &str,
    title: &str,
    body: &str,
    subreddit: &str,
    author: &str,
    permalink_id: &str,
    updated: &str,
) -> String {
    format!(
        r#"  <entry>
    <id>{id}</id>
    <title>{title}</title>
    <link href="https://www.reddit.com/r/{sub}/comments/{pid}/x/" />
    <author><name>/u/{author}</name></author>
    <category term="{sub}" label="r/{sub}" />
    <updated>{updated}</updated>
    <content type="html">&lt;p&gt;{body}&lt;/p&gt;</content>
  </entry>"#,
        id = thing_id,
        title = xml_escape(title),
        sub = subreddit,
        pid = permalink_id,
        author = author,
        updated = updated,
        body = xml_escape(body),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
