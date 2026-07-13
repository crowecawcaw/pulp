//! `pulp` command-line interface.
//!
//! Two kinds of commands, sharing code with the server so neither can drift:
//!
//! - **API commands** (`workspaces`, `monitors`, `mentions`, `notifications`,
//!   `channels`, `admin`) are a thin HTTP client over a running
//!   server. They serialize/deserialize the *same* DTO structs the Axum
//!   handlers use (`db::repos::traits`, `api::*`), so the CLI and the API
//!   contract are kept in sync by the compiler.
//! - **Exploration commands** (`query`) run the server's own `Collector`
//!   implementations in-process. `query` needs no server or database at
//!   all — it searches a channel live, which is how you trial keywords
//!   before creating a monitor.

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::io::{IsTerminal, Write};

pub mod app;
pub mod channels;
pub mod client;
pub mod config;
pub mod mentions;
pub mod monitors;
pub mod notifications;
pub mod query;
pub mod seed;
pub mod serve;
pub mod util;
pub mod workspaces;

use client::ApiClient;

const LONG_ABOUT: &str = "\
Pulp — open-source social listening. Monitors watch phrases across \
channels (reddit, hackernews, github, …); collectors ingest matching posts as \
mentions; every feed mention fans out to its workspace's notifications \
(webpush/webhook).

Most commands talk to a running Pulp server (start one with `pulp \
serve`; default http://127.0.0.1:3000, override with --server or \
PULP_SERVER). The exception is `pulp query`, which searches channels \
directly and works standalone — use it to explore a space and trial \
keywords/filters BEFORE creating monitors.

The server also writes its logs to ~/.pulp/server.log (PULP_HOME \
aware); `pulp logs` prints the path and tails it — the place to look when \
a channel reports collection errors.";

const EXAMPLES: &str = "\
EXAMPLES:
  Explore a space first (no server needed):
    pulp query \"desktop automation software\" --channel reddit
    pulp query \"desktop automation\" --channel hackernews --since 30d --exclude hiring
    pulp query \"Nimbus\" --channel reddit --subreddit QualityAssurance \\
        --exclude \"hiring\" --exact

  Set up monitoring (server running):
    pulp workspaces list
    pulp channels enable reddit
    pulp monitors create \"desktop automation\" --channel reddit --channel hackernews
    pulp admin collect reddit            # poll right now instead of waiting

  Read the feed:
    pulp mentions list --since 1d --unread
    pulp mentions mark-read <id> [<id>…]

  Configure the optional AI relevance filter (bring-your-own LLM endpoint):
    pulp config set base_url http://localhost:11434/v1
    pulp config set model llama3.2
    pulp config set enabled true
    pulp config test

  Debug a misbehaving channel (rate limits, auth errors):
    pulp channels list                    # error_message shows the last failure
    pulp logs --tail 100                  # server log (~/.pulp/server.log)

  Get notified — every feed mention fans out to the workspace's notifications:
    pulp notifications add-webhook --url https://example.com/hook --label slack
    pulp notifications list
    pulp admin notify                     # force a fan-out pass now

Output is human-readable on a terminal and JSON when piped; force either with
--json / --no-json.";

#[derive(Parser, Debug)]
#[command(
    name = "pulp",
    version,
    about = "Pulp social listening — server, API client, and channel exploration in one binary",
    long_about = LONG_ABOUT,
    after_help = EXAMPLES,
    propagate_version = true
)]
pub struct Cli {
    /// Base URL of the Pulp server (default: server.host:port from
    /// ~/.pulp/config.json, else http://127.0.0.1:3000)
    #[arg(long, global = true, env = "PULP_SERVER")]
    pub server: Option<String>,

    /// Print raw JSON (default: auto — JSON when stdout is not a terminal)
    #[arg(long, global = true)]
    pub json: bool,

    /// Force human-readable output (overrides the non-terminal JSON default)
    #[arg(long, global = true, conflicts_with = "json")]
    pub no_json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the Pulp server, or control a background one (start/stop/status)
    #[command(long_about = serve::LONG_ABOUT)]
    Serve {
        /// Omit to run in the foreground; start/stop/status manage a background server
        #[command(subcommand)]
        cmd: Option<serve::ServeCmd>,
    },

    /// Desktop launcher: run the server with a system-tray / menubar icon
    #[command(
        about = "Desktop launcher — run Pulp with a system-tray / menubar icon",
        long_about = app::LONG_ABOUT
    )]
    App(app::AppArgs),

    /// Print the OpenAPI spec for the HTTP API
    Openapi,

    /// Manage workspaces (top-level containers for monitors and notifications)
    #[command(subcommand, visible_alias = "workspace", visible_alias = "ws")]
    Workspaces(workspaces::WorkspacesCmd),

    /// Manage monitors — the phrases watched across channels
    #[command(subcommand, visible_alias = "monitor")]
    Monitors(monitors::MonitorsCmd),

    /// Read and triage the ingested mentions feed
    #[command(subcommand, visible_alias = "mention")]
    Mentions(mentions::MentionsCmd),

    /// Manage per-workspace notifications (webpush/webhook delivery endpoints)
    #[command(subcommand, visible_alias = "notification", long_about = notifications::LONG_ABOUT)]
    Notifications(notifications::NotificationsCmd),

    /// Configure channels (enable/disable, credentials, polling, backfill)
    #[command(subcommand, visible_alias = "channel")]
    Channels(channels::ChannelsCmd),

    /// View and edit configuration (currently: the optional AI relevance filter)
    #[command(long_about = config::LONG_ABOUT)]
    Config {
        /// Omit to list every option with its current and default value
        #[command(subcommand)]
        cmd: Option<config::ConfigCmd>,
    },

    /// Operational triggers on the server (run collectors/notifier now)
    #[command(subcommand)]
    Admin(AdminCmd),

    /// Locate and tail the server's log file (~/.pulp/server.log)
    Logs {
        /// Show the last N lines
        #[arg(long, default_value_t = 50)]
        tail: usize,
        /// Print only the log file path
        #[arg(long)]
        path: bool,
    },

    /// Populate the database with realistic demo data (no server needed)
    Seed(seed::SeedArgs),

    /// Search a channel live to trial keywords (no server needed)
    Query(query::QueryArgs),
}

#[derive(Subcommand, Debug)]
pub enum AdminCmd {
    /// Run one collection pass for a channel right now
    Collect {
        /// Channel name (e.g. reddit, hackernews, github)
        channel: String,
    },
    /// Run one notifier pass right now (fan feed mentions out to notifications)
    Notify,
    /// Re-collect history: fetch mentions back to a point in time
    Backfill {
        /// How far back: a relative age (30d, 12h), epoch seconds, or YYYY-MM-DD
        #[arg(long)]
        since: String,
        /// Limit to one channel (omit = all channels, runs in background)
        #[arg(long)]
        channel: Option<String>,
    },
}

/// Rotate-on-start size cap for `<home>/server.log`: past this the old log is
/// renamed to `server.log.old` (one generation kept) so the file an agent
/// tails stays bounded without a runtime rotation dependency.
const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;

/// The server's log file path: `<home>/server.log` next to config.json, so
/// `pulp logs` can locate it without a running server.
pub fn server_log_path() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::config::resolve_home()?.join("server.log"))
}

/// Set up `serve` logging: human format to stdout (ANSI) plus the same stream
/// ANSI-free appended to [`server_log_path`], so logs survive however the
/// process was launched. Falls back to stdout-only if the file can't be
/// opened. Default level is `info` when `RUST_LOG` is unset.
fn init_serve_logging() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let stdout_layer = tracing_subscriber::fmt::layer();

    let file_layer = server_log_path().ok().and_then(|path| {
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > LOG_ROTATE_BYTES {
                let _ = std::fs::rename(&path, path.with_extension("log.old"));
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => Some((
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::sync::Arc::new(file)),
                path,
            )),
            Err(e) => {
                eprintln!("warning: cannot open log file {}: {}", path.display(), e);
                None
            }
        }
    });

    match file_layer {
        Some((layer, path)) => {
            tracing_subscriber::registry()
                .with(filter)
                .with(stdout_layer)
                .with(layer)
                .init();
            tracing::info!("Logging to {}", path.display());
        }
        None => {
            tracing_subscriber::registry()
                .with(filter)
                .with(stdout_layer)
                .init();
        }
    }
}

/// Entry point used by `main`. Sets up logging appropriate to the command and
/// dispatches; `serve` never returns until the process is stopped.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    // Resolve the output mode once, here at the real entry point: explicit
    // --json / --no-json win, otherwise default to JSON when stdout is not a
    // terminal (programmatic use) and human text when it is. Done here rather
    // than in `execute` so integration tests, which call `execute` directly
    // with a captured buffer, keep their default human-readable output.
    let mut cli = cli;
    if !cli.json && !cli.no_json {
        cli.json = !std::io::stdout().is_terminal();
    }

    match &cli.command {
        // Foreground serve: full server logging, never returns until stopped.
        Command::Serve { cmd: None } => {
            init_serve_logging();
            serve::foreground().await
        }
        // serve start/stop/status: short control ops that print to stdout.
        Command::Serve { cmd: Some(sub) } => {
            let mut out = std::io::stdout().lock();
            serve::control(sub, cli.json, &mut out)
        }
        // `app` reaches here only for the headless fallback: either this binary
        // was built without the `tray` feature (in which case `main` never
        // intercepted it), or `--no-tray` was passed. Both run the server in the
        // foreground under the `app` name. (The full tray path is handled in
        // `main` before the runtime is built — see `app::run_blocking`.)
        Command::App(_args) => {
            init_serve_logging();
            #[cfg(not(feature = "tray"))]
            eprintln!(
                "this build was compiled without desktop tray support; rebuild with \
                 `--features tray`, or use `pulp serve`"
            );
            serve::foreground().await
        }
        _ => {
            // Client/exploration commands: surface collector warnings on
            // stderr but stay quiet otherwise (RUST_LOG still overrides).
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
                )
                .init();
            let mut out = std::io::stdout().lock();
            execute(cli, &mut out).await
        }
    }
}

/// Dispatch every non-`serve` command, writing output to `out` (injected so
/// integration tests can capture it). `serve` is handled in [`run`].
pub async fn execute(cli: Cli, out: &mut dyn Write) -> anyhow::Result<()> {
    let client = ApiClient::new(cli.server.as_deref());
    let json = cli.json;
    match cli.command {
        Command::Serve { .. } => unreachable!("serve is dispatched in run()"),
        Command::App(_) => unreachable!("app is dispatched in run()"),
        Command::Openapi => {
            use utoipa::OpenApi;
            writeln!(out, "{}", crate::api::ApiDoc::openapi().to_pretty_json()?)?;
            Ok(())
        }
        Command::Workspaces(cmd) => workspaces::run(cmd, &client, json, out).await,
        Command::Monitors(cmd) => monitors::run(cmd, &client, json, out).await,
        Command::Mentions(cmd) => mentions::run(cmd, &client, json, out).await,
        Command::Notifications(cmd) => notifications::run(cmd, &client, json, out).await,
        Command::Channels(cmd) => channels::run(cmd, &client, json, out).await,
        Command::Config { cmd } => config::run(cmd, &client, json, out).await,
        Command::Admin(cmd) => run_admin(cmd, &client, out).await,
        Command::Logs { tail, path } => run_logs(tail, path, json, out),
        Command::Seed(args) => seed::run(args, json, out).await,
        Command::Query(args) => query::run(args, &client, json, out).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// clap's self-check: catches conflicting flags, bad defaults, etc.
    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_representative_commands() {
        let cli = Cli::try_parse_from([
            "pulp",
            "--server",
            "http://localhost:9999",
            "--json",
            "monitors",
            "create",
            "desktop automation",
            "--term",
            "pywinauto",
            "--channel",
            "reddit",
            "--channel",
            "hackernews",
            "--exact",
            "--exclude",
            "hiring",
        ])
        .unwrap();
        assert!(cli.json);
        assert_eq!(cli.server.as_deref(), Some("http://localhost:9999"));
        match cli.command {
            Command::Monitors(monitors::MonitorsCmd::Create {
                terms,
                extra_terms,
                channels,
                exact,
                exclude,
                ..
            }) => {
                assert_eq!(terms, ["desktop automation"]);
                assert_eq!(extra_terms, ["pywinauto"]);
                assert_eq!(channels, ["reddit", "hackernews"]);
                assert!(exact);
                assert_eq!(exclude, ["hiring"]);
            }
            other => panic!("unexpected parse: {:?}", other),
        }

        let cli = Cli::try_parse_from([
            "pulp",
            "query",
            "desktop automation",
            "--channel",
            "reddit",
            "--subreddit",
            "QualityAssurance",
        ])
        .unwrap();
        match cli.command {
            Command::Query(args) => {
                assert_eq!(args.channels, ["reddit"]);
                assert_eq!(args.subreddits, ["QualityAssurance"]);
                assert_eq!(args.since, "7d");
            }
            other => panic!("unexpected parse: {:?}", other),
        }
    }

    #[test]
    fn query_requires_a_channel_and_aliases_work() {
        assert!(Cli::try_parse_from(["pulp", "query", "x"]).is_err());
        // Singular aliases resolve to the same subcommands.
        assert!(Cli::try_parse_from(["pulp", "monitor", "list"]).is_ok());
        assert!(Cli::try_parse_from(["pulp", "ws", "list"]).is_ok());
        // Bare invocation prints help (error from parse, not a panic).
        assert!(Cli::try_parse_from(["pulp"]).is_err());
    }

    /// Regression test: `--help`/`long_about` text used to show `pulp query
    /// ... --filter '...'`, but `query` has no `--filter` flag (and there's
    /// no `pulp filter`/`pulp alerts` command at all) — a user or agent
    /// pasting the binary's own example would get an "unexpected argument"
    /// error. Every `pulp query ...` line actually printed in EXAMPLES must
    /// parse as real flags, and neither help string may reference the
    /// removed `filter`/`alerts` commands.
    #[test]
    fn help_examples_are_real_commands() {
        assert!(
            Cli::try_parse_from([
                "pulp",
                "query",
                "Nimbus",
                "--channel",
                "reddit",
                "--subreddit",
                "QualityAssurance",
                "--exclude",
                "hiring",
                "--exact",
            ])
            .is_ok(),
            "the query example in EXAMPLES must parse with query's actual flags"
        );
        for text in [LONG_ABOUT, EXAMPLES] {
            assert!(
                !text.contains("--filter"),
                "no --filter flag exists on any command"
            );
            assert!(
                !text.contains("pulp filter"),
                "the `filter` command was removed"
            );
            assert!(
                !text.contains("pulp alerts"),
                "the `alerts` command was removed"
            );
        }
    }
}

async fn run_admin(cmd: AdminCmd, client: &ApiClient, out: &mut dyn Write) -> anyhow::Result<()> {
    match cmd {
        AdminCmd::Collect { channel } => {
            util::ensure_known_channel(&channel)?;
            client
                .post_no_response(&format!("/api/admin/collect/{}", channel))
                .await?;
            // The pass "succeeding" only means it ran; the channel records
            // upstream failures (rate limits, auth) in error_message. Surface
            // that here instead of printing a misleading all-clear.
            let cfg: Option<crate::db::repos::traits::ChannelConfig> =
                client.get(&format!("/api/channels/{}", channel)).await.ok();
            match cfg.and_then(|c| c.error_message) {
                Some(err) => {
                    writeln!(
                        out,
                        "collection pass for '{}' ran, but the channel reported an error:",
                        channel
                    )?;
                    writeln!(out, "  {}", err)?;
                    writeln!(
                        out,
                        "some or all requests failed — mentions from requests that succeeded \
                         were still ingested (`pulp mentions list --since 1h`)"
                    )?;
                }
                None => writeln!(
                    out,
                    "collection pass for '{}' complete — see `pulp mentions list --since 1h`",
                    channel
                )?,
            }
        }
        AdminCmd::Notify => {
            client.post_no_response("/api/admin/notify").await?;
            writeln!(out, "notifier pass complete")?;
        }
        AdminCmd::Backfill { since, channel } => {
            let since_ts = util::parse_since(&since)?;
            let body = crate::api::admin::BackfillRequest {
                channel: channel.clone(),
                since: since_ts,
            };
            let status = client.post_status("/api/admin/backfill", &body).await?;
            if status == 202 {
                writeln!(
                    out,
                    "backfill accepted for all channels (runs in background)"
                )?;
            } else {
                writeln!(
                    out,
                    "backfill complete for '{}'",
                    channel.as_deref().unwrap_or("all")
                )?;
            }
        }
    }
    Ok(())
}

/// `pulp logs` — print the server log path and its last `tail` lines.
/// Reads the file directly (no server needed): the path is fixed by the app
/// home, so this works even when the server is down or was started by
/// another process.
fn run_logs(tail: usize, path_only: bool, json: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    let log_path = server_log_path()?;
    let exists = log_path.is_file();

    if path_only && !json {
        writeln!(out, "{}", log_path.display())?;
        return Ok(());
    }

    let lines: Vec<String> = if exists && !path_only {
        let content = std::fs::read_to_string(&log_path)
            .with_context(|| format!("reading {}", log_path.display()))?;
        let all: Vec<&str> = content.lines().collect();
        all.iter()
            .skip(all.len().saturating_sub(tail))
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    if json {
        util::print_json(
            out,
            &serde_json::json!({
                "path": log_path.display().to_string(),
                "exists": exists,
                "lines": lines,
            }),
        )?;
        return Ok(());
    }

    writeln!(out, "log file: {}", log_path.display())?;
    if !exists {
        writeln!(
            out,
            "(no log file yet — it is created when `pulp serve` runs on this machine)"
        )?;
        return Ok(());
    }
    for line in &lines {
        writeln!(out, "{}", line)?;
    }
    Ok(())
}
