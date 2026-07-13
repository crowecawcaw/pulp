# Reddit

Source: `backend/src/collectors/reddit.rs` (feed parsing shared with other RSS
sources in `backend/src/collectors/rss_parse.rs`).

**Auth:** none. Reddit's `*.json` search API returns `403 Forbidden` to
unauthenticated clients regardless of `User-Agent`, so this collector uses
Reddit's public, unauthenticated RSS/Atom feeds (`*.rss`) instead. No OAuth
app, client id, or secret is required — `credentials.user_agent` is a courtesy
string sent on requests, not a credential.

**Throttling design — global search only, OR-batched:** every monitor is
served by Reddit's global `search.rss?q="a" OR "b" OR …`, with *all* monitors'
terms OR-batched into one query (or a few, chunked under Reddit's query-length
cap) per collection pass; each result is then matched client-side against
every monitor. This is deliberate — the collector does **not** use the
multireddit `new.rss` firehose or per-subreddit `restrict_sr` search, because
both multiply request count against Reddit's per-IP rate limit, and the
firehose is the surface Reddit throttles hardest. The trade-off: global search
is indexed (a new post appears with some latency, not instantly like a
firehose) and not perfectly exhaustive — acceptable for keyword listening on
distinctive terms, but worth knowing if a mention seems to "arrive late."

`subreddits` (in `credentials`) is applied as a client-side include filter on
top of those global-search results (empty = all of Reddit); `exclude_subreddits`
/ `exclude_authors` are client-side exclude filters, applied after parsing.

Reddit is currently the only channel that runs through the shared adaptive
throttle in `backend/src/ratelimit.rs` (AIMD-style: backs off on `429`,
recovers gradually) via the `TargetedCollector` runner in
`backend/src/collectors/scheduler.rs` — Hacker News and GitHub use the plain
per-pass `Collector::fetch` path.

**Caveat:** Reddit RSS carries no `score` / `num_comments`, so
`platform_meta.score` is `null` for Reddit mentions, and any criteria that
compares on `score` can never match them.

**Legacy fields:** there is no more per-subreddit search mode or multireddit
firehose mode — `mode` / `global_search` keys left over in older stored
credentials are silently ignored rather than erroring.
