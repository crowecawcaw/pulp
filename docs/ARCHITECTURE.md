# Pulp — Architecture

**Pulp** is an open-source, self-hostable social listening tool for monitoring mentions across developer communities (Hacker News, Reddit, GitHub). It aggregates them into a unified workspace-scoped feed, optionally filters with AI, and broadcasts feed-visible mentions to all workspace notifications via webpush or webhooks.

**Single-process backend** (Rust + Axum + SQLite) with collector and notifier loops. **React frontend** (TypeScript + Tailwind). Distributed as a native binary; no Docker required.

For operational detail and configuration, see **AGENTS.md** (source of truth for running behavior). For the current schema, see **backend/migrations/0001_initial.sql**. For API surface, see **backend/openapi.json** and **frontend/src/api/types.ts**.

---

## Goals & Non-Goals

**Goals:**
- Monitor free/open APIs (Hacker News, Reddit, GitHub) with zero or freely-obtainable credentials
- Unified mention feed scoped by workspace
- Optional ingest-time AI relevance filter (bring your own OpenAI-compatible LLM endpoint)
- Simple notification fan-out: every feed-visible mention broadcasts to all per-workspace destinations
- Multi-workspace for project separation (e.g., "My Product", "Competitor Watch")
- REST API + agent-friendly CLI; runs as a single statically-linked binary (an MCP server adapter is a possible future addition, not built today)

**Non-Goals:**
- Paid/restricted APIs (Twitter/X, LinkedIn, TikTok)
- Multi-user accounts or team features
- Criteria-based alert routing (see AGENTS.md: the notifier fans out all feed-visible mentions to every notification in the workspace)

---

## Supported Channels

Three collectors ship by default: **hackernews**, **reddit**, **github**.

All are free or zero-auth (Reddit needs no OAuth app; GitHub works unauthenticated but a PAT lifts the rate limit). See `docs/channels/` for per-channel design and AGENTS.md § "Channels" for how monitor-level `channel_settings` override global `channel_configs.credentials` at collection time (useful for, e.g., Reddit subreddits or GitHub repo filters).

---

## High-Level Architecture

```
┌─ React Frontend ─────────────┐
│  Feed | Workspaces | Settings│
└──────────┬────────────────────┘
           │ REST + SSE
   ┌───────▼──────────────┐
   │   Axum HTTP Server   │
   │  (single Rust binary)│
   │                      │
   │ ┌─ Collector Loop ─┐ │
   │ │ (tokio tasks,    │ │
   │ │  one per channel)│ │
   │ └──────────────────┘ │
   │                      │
   │ ┌─ AI Filter ─────┐  │
   │ │ (optional,      │  │
   │ │  ingest-time)   │  │
   │ └──────────────────┘  │
   │                       │
   │ ┌─ Notifier Loop ──┐  │
   │ │ (fan-out to      │  │
   │ │  workspace       │  │
   │ │  notifications)  │  │
   │ └──────────────────┘  │
   │                       │
   │   SQLite (single file)│
   └───────────────────────┘
```

**Collector Loop**: One tokio task per channel (default 15-min poll interval). Fetches mentions, deduplicates by `(monitor_id, channel, external_id)`, inserts new rows into the mentions table scoped to their monitor.

**AI Filter** (optional, see AGENTS.md § "Ingest-time AI filter"; config in [docs/CONFIGURATION.md](CONFIGURATION.md)): When a monitor has `ai_filter_prompt` set, newly collected mentions are held with `ai_verdict = 'pending'`, hidden from the feed. An async worker judges them against the configured endpoint; accepted mentions are broadcast, rejected mentions are kept but hidden (soft filter).

**Notifier Loop**: Periodically scans feed-visible mentions (where `ai_verdict` is NULL or `accepted`). For each mention, broadcasts it to every per-workspace notification (webpush or webhook) that hasn't yet received it (gated by `mentions.notified_at`).

**Repository Layer**: All DB access via trait-based repository interfaces; trait implementations live in `backend/src/db/repos/`. SQLite is the default; Postgres can be swapped later by implementing the same traits.

---

## Data Model & Key Design Decisions

**Multi-workspace, single-tenant.** Each workspace has its own monitors and notifications; channel credentials are global (connect once, usable from all workspaces).

**Monitors, not keywords.** A monitor specifies `terms` (match-ANY) and `exclude_terms` (match-NONE) to watch across selected channels. Mentions are scoped to their monitor via `mentions.monitor_id`.

**Dedup is per-monitor per channel:** `UNIQUE(monitor_id, channel, external_id)`. The same external post is stored once per matching monitor, so read state, AI verdicts, and notifications stay scoped per workspace.

**No alerts or criteria matching.** Monitors (+ optional AI filter) decide what enters the feed. The notifier simply fans out every feed-visible mention to all workspace notifications — no further filtering, no per-monitor toggles, no frequency/digest modes.

**AI filter is optional and ingest-time only.** When configured, newly collected mentions are held pending (`ai_verdict = 'pending'`), hidden from feed and SSE stream. An async worker judges each with the custom prompt; accepted mentions are then broadcast. See AGENTS.md § "Ingest-time AI filter" for error handling and failure modes.

**Notifications scope by workspace.** Each notification (webpush or webhook) is tied to a workspace. Every feed-visible mention in that workspace fans out to all its notifications once (`notified_at` gate prevents re-delivery).

**Single SQLite file** — no Postgres dependency for self-hosting simplicity. Credentials stored as JSON in `channel_configs.credentials`. See backend/migrations/0001_initial.sql for the full schema.

**Code-first API contract** (utoipa OpenAPI). The Rust backend is the source of truth; frontend types are generated from backend/openapi.json via `npm run gen:api`. See AGENTS.md § "API contract: Rust is the source of truth" for the workflow.

**Web Push is pure-Rust, no native build deps.** RFC 8291 message encryption
(`aes128gcm`) and RFC 8292 VAPID (ES256 JWT) are implemented with RustCrypto
crates (`p256`/`hkdf`/`aes-gcm`) in `backend/src/notifier/webpush.rs` rather
than the obvious `web-push` crate, which pulls in OpenSSL via `ece` — that
would break this project's rustls/ring, single-statically-linked-binary
stance. See [docs/WEBPUSH.md](WEBPUSH.md) for the operational details
(VAPID persistence, subscription model, endpoints).

---

## Supported Fictional Scenarios

All fabricated data (demo fixtures, test data) uses two canonical scenarios defined in the [`fixtures`](../.claude/skills/fixtures/SKILL.md) skill:

1. **Nimbus** — A developer-tools company monitoring their product.
2. **Fern** — An open-source SSG maintainer tracking buzz and issues.

These fictional names appear consistently in seed data (backend/src/cli/seed.rs), frontend demo fixtures, and documentation examples.

---

## See Also

- **AGENTS.md** — Operational guide; running the app, CLI, testing, deploy, and architectural rationale
- **docs/CONFIGURATION.md** — `config.json` schema, precedence, runtime editing
- **docs/WEBPUSH.md** — Web Push destination: VAPID, subscription model, endpoints
- **backend/migrations/0001_initial.sql** — Current database schema
- **backend/src/collectors/mod.rs** — Collector trait and channel list (`CHANNELS`)
- **docs/channels/** — Per-channel auth, throttling, and filtering design
- **backend/src/ai_filter.rs** — Ingest-time AI filter logic
- **backend/src/notifier/mod.rs** — Notification fan-out
- **backend/openapi.json** — REST API spec (regenerate via `cargo run -- --dump-openapi`)
- **frontend/src/api/types.ts** — TypeScript domain types (regenerate via `npm run gen:api`)
