# Hacker News

Source: `backend/src/collectors/hackernews.rs`

**Auth:** none. Uses the [Algolia HN Search API](https://hn.algolia.com/api),
which is free and unauthenticated. The channel's `credentials` JSON is unused —
no fields are read for this collector.

**Why one request per term:** Algolia's `query` full-text search has no
boolean OR — it defaults to requiring every word (AND), and `advancedSyntax`
only adds phrase-quoting and `-exclusion`. Since a monitor matches on *any* of
its terms, a single space-joined query would under-recall (only hits
containing every term would surface). The collector instead issues one search
per distinct term and unions the deduped results — a true OR, at the cost of
one request per term. `matches_monitor` still re-filters client-side for exact
match semantics.

**Rate limiting:** a `429` response is treated as a hard stop for the pass
(`RateLimited` marker error aborts remaining requests rather than continuing
to hammer an already-throttled endpoint) — no adaptive backoff here since
Algolia's limits are generous enough that this hasn't needed one.

**Base URL override:** `HACKERNEWS_BASE_URL` env var, used by integration
tests to point at a local mock server.
