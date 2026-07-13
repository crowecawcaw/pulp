# Channels

Pulp ships three collectors, each implementing the `Collector` trait in
`backend/src/collectors/`. `CHANNELS` in `backend/src/collectors/mod.rs` is the
single source of truth for which channels exist — the CLI's channel validation
and the background poll loop (`spawn_all`) both derive from it, so a new
collector can't drift from what's advertised.

- [Hacker News](hackernews.md)
- [Reddit](reddit.md)
- [GitHub](github.md)

For the exact shape of each channel's `credentials` JSON, read the doc
comments on the collector itself (`backend/src/collectors/{reddit,github_filter}.rs`)
or the CLI/API DTOs — these pages cover *design*, not a field-by-field schema.

## Per-monitor channel scoping

Each channel has one global credentials blob (`channel_configs.credentials`)
plus per-monitor overrides (`monitors.channel_settings`, keyed by channel
name, e.g. `{"reddit": {"subreddits": ["accessibility"]}}`). At collection
time `collectors::merged_credentials` shallow-merges the monitor's override
JSON *over* the global credentials (monitor keys win), so scoping normally
lives on the monitor and the global channel config can stay empty. Any key a
collector reads from its credentials JSON can be overridden this way.
