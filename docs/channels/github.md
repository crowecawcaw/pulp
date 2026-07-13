# GitHub

Source: `backend/src/collectors/github.rs` (glob filtering in
`backend/src/collectors/github_filter.rs`).

**Auth:** optional. `credentials.token` (a GitHub PAT, no special scopes
needed for public search) is sent as a bearer token when present; the search
API works unauthenticated too but at GitHub's much lower unauthenticated rate
limit, so a token is recommended for anything beyond light use. A `429` is
treated as a hard stop for the pass, same as Hacker News — no adaptive backoff.

**Query construction:** the monitor's terms are quoted (exact phrase) and
OR-joined into one GitHub code/issue search query, plus a
`created:>=<date>` qualifier when resuming from a watermark — GitHub's search
API has no working `since` param, so this is best-effort and the collector
also re-floors client-side on `created_at`. Results are capped at GitHub's
1000-result / 10-page search ceiling in addition to the shared backfill cap.

**Filtering design:** `ignore_repos` / `ignore_orgs` / `ignore_authors` /
`only_repos` (in `credentials`) support `*` globs (case-insensitive, `*`
matches any sequence including `/`) via `github_filter::glob_match` — a
hand-rolled matcher, not a regex crate, since the only pattern needed is
prefix/suffix/contains wildcarding on repo and org names. `state_filter`
(`open` / `closed` / `all`, defaults to `open`) maps to GitHub's `is:` search
qualifier.
