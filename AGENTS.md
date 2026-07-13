# Pulp — Agent & Contributor Guide

Pulp is an open-source self-hostable social listening tool (Octolens clone), B2B/developer focus.

## Stack

- **Backend**: Rust + Axum + SQLite (SQLx) — lives in `backend/`
- **Frontend**: React 19 + TypeScript + Vite + Tailwind + shadcn/ui — lives in `frontend/`
- **Auth**: none — the API and UI are open (MVP is meant to run behind your own network controls)
- **Collectors**: async Tokio tasks, one per channel, poll on configurable interval
- **AI relevance filter**: optional, off by default — bring your own OpenAI-compatible LLM endpoint; see [Configuration](docs/CONFIGURATION.md)

## Running locally

```bash
# Backend (requires Rust stable)
cd backend && cargo run -- serve

# Frontend (requires Node 20+)
cd frontend && npm install && npm run dev
```

- `pulp` is one binary; `serve` runs the server (bare `pulp` prints help).
- `serve` binds a **fixed, singleton address** (`server.host:port` / `BIND`); state tracked in `<home>/pulp.pid`.
- `pulp serve` runs in the foreground; `pulp serve start`/`stop`/`status` manage a background instance (idempotent, agent-friendly).
- Port already taken → interactive terminals get prompted to stop the occupant, non-interactive runs print why and exit non-zero.
- See [The pulp CLI](#the-pulp-cli) for client/exploration commands.

## Configuration

Persistent state lives in `~/.pulp` (`PULP_HOME` to override) — `config.json`,
the SQLite DB, `pulp.pid`, `vapid.json`. Schema, precedence, and how to edit
at runtime: **[docs/CONFIGURATION.md](docs/CONFIGURATION.md)** (source of
truth is `backend/src/config.rs` — don't duplicate the schema here).

## Architecture highlights

- `backend/src/collectors/` — one file per data source; all implement the `Collector` trait
- `backend/src/api/` — Axum handlers (no auth — see [Stack](#stack))
- `backend/src/server.rs` — server startup wiring (config, DB, collectors, router); `main.rs` is a thin clap dispatcher
- `backend/src/db/repos/` — repository traits + SQLite impls; all business logic goes through traits
- `frontend/src/pages/` — one file per route; shared state via Zustand
- `frontend/src/api/` — typed fetch client (`client.ts`) + domain types (`types.ts`)

### Channels

Three collectors ship by default (`CHANNELS` in `backend/src/collectors/mod.rs`
is the source of truth): **hackernews**, **reddit**, **github**. Per-channel
auth/throttling/filtering design lives in
[`docs/channels/`](docs/channels/README.md), including how
`monitors.channel_settings` overrides global `channel_configs.credentials` at
collection time.

### Ingest-time AI filter (feed gating)

- Monitor has `ai_filter_prompt` + a judge configured → new mentions insert as `ai_verdict = 'pending'`, hidden from feed/SSE until judged.
- `backend/src/ai_filter.rs` (~15s loop) judges pending mentions, records `accepted`/`rejected` + a one-sentence `ai_reason`; accepted mentions broadcast then.
- `GET /api/mentions` defaults to feed-visible (`ai_verdict` NULL or `accepted`); `ai=all|pending|accepted|rejected` sees the rest. Rejected mentions are kept, never deleted.
- Failure policy: judge unavailable → stays pending, retried; 5 consecutive judge errors on one mention → fails open into the feed; AI disabled entirely → no gating, stranded pending mentions accepted with an explanatory reason.
- This is the only matching Pulp does — the notifier (`backend/src/notifier/mod.rs`) fans out every feed-visible mention to every notification in its workspace, no further criteria.

## Web Push notifications

Browser/PWA push is a `webpush` notification destination (alongside
`webhook`), backed by pure-Rust RFC 8291/8292 crypto — no third-party
account, no native build deps. Details, VAPID lifecycle, endpoints:
**[docs/WEBPUSH.md](docs/WEBPUSH.md)**.

## API contract: Rust is the source of truth

Code-first via [`utoipa`](https://docs.rs/utoipa): handlers carry
`#[utoipa::path(...)]`, DTOs derive `ToSchema`, aggregated in
`backend/src/api/mod.rs` (`ApiDoc`). The spec isn't served by the binary
(keeps the build network-free); a checked-in copy lives at
`backend/openapi.json`.

```bash
# after changing a handler/DTO:
cd backend && cargo run -- --dump-openapi > openapi.json
cd frontend && npm run gen:api:file   # regenerates types.gen.ts from ../backend/openapi.json
```

- `frontend/src/api/types.gen.ts` is generated — never edit it; hand-maintain narrowings in `types.ts`.
- CI should fail on drift (regenerate both files, assert `git diff` empty).
- `frontend/.npmrc` sets `legacy-peer-deps=true` (openapi-typescript's `typescript@^5` peer vs. this repo's TS 6); `@testing-library/dom` is pinned explicitly to survive that.

## The pulp CLI

One binary; every non-`serve` subcommand is an agent-friendly CLI
(`--help` on each). Output is human-readable on a TTY, JSON when piped
(`--json`/`--no-json` to force). Implementation: `backend/src/cli/`.

- **API commands** (`workspaces`, `monitors`, `mentions`, `notifications`, `channels`, `config`, `admin`, `openapi`) — thin HTTP client. Server resolution: `--server` > `PULP_SERVER` > `config.json` `server.host:port` > `http://127.0.0.1:3000`.
- **Exploration**: `pulp query <phrase> --channel <ch>` runs a collector in-process (no server/DB) to iterate on keywords before saving a monitor.
- **Local-DB**: `pulp seed [--reset]` opens the DB directly and writes a demo workspace (monitors, enriched mentions, notifications) mirroring the frontend fixtures; `--reset` makes it repeatable.

### Sync rules — the compiler enforces the CLI/API contract

- Never define parallel request/response structs under `cli/` — reuse the server's DTOs (`db::repos::traits`, `api::*`) on both sides.
- `query` runs the same `Collector` impls (`collectors::make_collector`) as the server's poll loop.
- `collectors::CHANNELS` is the canonical channel list; kept honest by `collectors::tests::channels_list_matches_factory`. Adding a collector means updating both the factory and `CHANNELS`.
- CLI tests (`backend/tests/test_cli.rs`) drive `pulp::cli::execute` against the same in-process server as the API tests (`tests/common::spawn_app`); handlers take `&mut dyn Write` to stay testable without spawning processes.

## Test & demo data

All fabricated data (seed, frontend fixtures, test fixtures, mock responses,
doc examples) must draw from the two canonical fictional scenarios in the
[`fixtures`](.claude/skills/fixtures/SKILL.md) skill — no real names/products.

## Testing

**All new features must include tests.**

### Backend

Prefer integration tests over mocking internals: `tests/common::spawn_app()`
starts the full server on an in-memory DB, hit it with `reqwest`. For
external APIs, spin up `httpmock::MockServer` and point the relevant
`*_BASE_URL` env var at it (`#[serial]` when mutating env vars). Pure-function
unit tests live in `#[cfg(test)]` blocks alongside the code.

```bash
cd backend && cargo test
cd backend && cargo test -- --ignored --nocapture   # network-required tests
```

### Frontend

Vitest + `@testing-library/react`; mock `@/api/client` and
`@/components/ui/toast`. Tests live in `src/tests/`.

```bash
cd frontend && npm test
```

### Visual changes — verify in a browser

Don't ship UI from code reading alone — screenshot before/after with
[`agent-browser`](https://agent-browser.dev) and report the screenshots back
to the user.

```bash
npm install -g agent-browser && agent-browser install   # or --executable-path /opt/pw-browsers/chromium-*/chrome-linux/chrome
cd frontend && npm run dev                               # http://localhost:5173
agent-browser open "http://localhost:5173/feed?demo"     # ?demo = backend-free fixtures
agent-browser set device "iPhone 14"                      # emulate mobile; reload after
agent-browser screenshot /tmp/after.png                   # then Read the PNG
```

Scroll the in-app container, not the window:
`agent-browser eval "document.querySelector('.app-content').scrollTop = 99999"`.

## Key design decisions

See `docs/ARCHITECTURE.md` for the full spec, and `docs/channels/` for
per-channel auth/throttling/filtering design.

## Frontend styling

Single source of truth: **`frontend/src/theme.css`**. Retheme by editing only
the two base vars at the top of `:root` (`--c-accent`, `--c-ink`) — everything
else derives via `color-mix()`.

`index.css` maps shadcn/Radix internals to HSL vars in `@layer base` (e.g.
`--accent`), which can shadow same-named `theme.css` vars — use
`var(--c-accent)` for brand fills, never `var(--accent)`. `--accent-l/-t/-d`,
`--ink`, `--mid`, `--faint`, `--border` are safe (no shadcn equivalents).

1. **No Tailwind utility classes in component JSX** (`py-3`, `text-sm`, …) — use semantic component classes (`btn`, `card`, `field-group`) instead.
2. **Exception: responsive visibility** — Tailwind breakpoint toggles (`hidden`, `lg:hidden`, `sm:hidden`) are fine since they control show/hide, not style.
3. **New UI gets named classes in `theme.css`** — never inline spacing/color/font values in JSX.
4. **Name classes by component role**, not property: `.mention__actions` not `.flex-row-xs-bold`.
