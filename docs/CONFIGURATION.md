# Configuration

Pulp keeps persistent state in an app home directory (`~/.pulp`, override
with `PULP_HOME`):

```text
~/.pulp/
├── config.json     user-editable config; written with explicit defaults on first run
├── pulp.db         default SQLite database location
├── pulp.pid        PID of the running `serve` (singleton guard)
└── vapid.json      Web Push VAPID keypair, generated on first run (see docs/WEBPUSH.md)
```

## Schema

Canonical schema: `FileConfig` and friends in `backend/src/config.rs` (all
fields optional; unknown fields ignored). Shape as of writing:

```json
{
  "server": { "host": "127.0.0.1", "port": 3000 },
  "database_path": null,
  "ai_filter": {
    "enabled": false,
    "base_url": "http://localhost:11434/v1",
    "model": "",
    "api_key": null
  }
}
```

- `database_path: null` → `<home>/pulp.db`.
- `ai_filter` — bring-your-own OpenAI-compatible LLM endpoint (local Ollama,
  LM Studio, llama-server, vLLM, or a hosted provider with `api_key` as the
  bearer token). Activates only when `enabled: true` and both `base_url` and
  `model` are non-empty; otherwise `ai::judge_from_config` returns no judge
  (see AGENTS.md § "Ingest-time AI filter" for what that means for new
  mentions). Implementation: `backend/src/ai/local.rs` (`OpenAiCompatJudge`).

## Precedence

Built-in defaults < `config.json` < environment variables:
`PULP_HOME`, `DATABASE_URL` (→ `database_path`), `BIND` (→ `server.host:port`),
`PULP_LLM_BASE_URL` / `PULP_LLM_MODEL` / `PULP_LLM_API_KEY` (→ matching
`ai_filter` fields). Covered by unit tests in `config::tests`.

## Editing at runtime

No restart needed — via the Settings page, `pulp config` CLI
(`list`/`get`/`set`/`test`), or `GET`/`PUT /api/config/ai`. A `PUT` persists
to `config.json` and hot-swaps the live judge (held behind a lock in
`AppState`). The API never returns the stored `api_key`, only whether one is
set. Writes go through `Config::save_ai_filter`.
