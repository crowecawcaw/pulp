use clap::Subcommand;
use std::io::Write;

use crate::api::channels::{
    BackfillBody, BackfillResult, ChannelBody, CleanupBody, CleanupResponse,
};
use crate::db::repos::traits::ChannelConfig;

use super::client::ApiClient;
use super::util;

const CREDS_HELP: &str = "\
CREDENTIALS (per channel, all fields optional — see AGENTS.md for details):
  reddit:   {\"user_agent\":\"...\",\"subreddits\":[\"sub1\"],
             \"exclude_subreddits\":[],\"exclude_authors\":[]}        (no token needed)
  github:   {\"token\":\"gh_pat_...\",\"ignore_repos\":[\"org/*\"],\"ignore_orgs\":[],
             \"ignore_authors\":[],\"only_repos\":[],\"state_filter\":\"open\"}
  hackernews: usually {}

EXAMPLES:
  pulp channels enable reddit
  pulp channels set reddit --credentials '{\"subreddits\":[\"QualityAssurance\"]}'
  pulp channels set github --credentials @github-creds.json --poll-interval 600";

#[derive(Subcommand, Debug)]
pub enum ChannelsCmd {
    /// List every channel's config and polling status
    List,
    /// Show one channel's config
    Get {
        /// Channel name
        channel: String,
    },
    /// Create or update a channel config (unset flags keep current values)
    #[command(after_help = CREDS_HELP)]
    Set {
        /// Channel name
        channel: String,
        /// true/false: turn collection on or off
        #[arg(long)]
        enabled: Option<bool>,
        /// Channel-specific credentials JSON or @file.json
        #[arg(long)]
        credentials: Option<String>,
        /// Poll interval in seconds (default 900 on first configure)
        #[arg(long)]
        poll_interval: Option<i64>,
    },
    /// Enable collection for a channel (shorthand for set --enabled true)
    Enable {
        /// Channel name
        channel: String,
    },
    /// Disable collection for a channel
    Disable {
        /// Channel name
        channel: String,
    },
    /// Remove already-ingested mentions that the channel's ignore filters
    /// would now exclude (github). Dry-run by default; --apply deletes.
    Cleanup {
        /// Channel name
        channel: String,
        /// Actually delete (default is a dry-run preview)
        #[arg(long)]
        apply: bool,
    },
    /// Re-collect this channel back N days
    Backfill {
        /// Channel name
        channel: String,
        #[arg(long, default_value_t = 7)]
        days: u32,
    },
}

fn print_channel(out: &mut dyn Write, c: &ChannelConfig) -> std::io::Result<()> {
    writeln!(
        out,
        "{:<14}  {:<8}  {:>6}s  {:<17}  {}",
        c.channel,
        if c.enabled { "enabled" } else { "disabled" },
        c.poll_interval,
        util::fmt_ts(c.last_polled_at),
        c.error_message.as_deref().unwrap_or("-")
    )
}

/// Upsert a channel, preserving any field the caller didn't set. The raw API
/// replaces the whole config (omitted credentials reset to `{}`), so the CLI
/// merges with the current state first.
async fn set_channel(
    client: &ApiClient,
    channel: &str,
    enabled: Option<bool>,
    credentials: Option<String>,
    poll_interval: Option<i64>,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    util::ensure_known_channel(channel)?;
    let current: Option<ChannelConfig> =
        client.get(&format!("/api/channels/{}", channel)).await.ok();
    let creds_value = match credentials.as_deref() {
        Some(arg) => {
            let text = match arg.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("reading credentials file {}: {}", path, e))?,
                None => arg.to_string(),
            };
            serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("--credentials is not valid JSON: {}", e))?
        }
        None => current
            .as_ref()
            .map(|c| c.credentials.clone())
            .unwrap_or_else(|| serde_json::json!({})),
    };
    let body = ChannelBody {
        enabled,
        credentials: Some(creds_value),
        poll_interval: poll_interval.or(current.as_ref().map(|c| c.poll_interval)),
    };
    let updated: ChannelConfig = client
        .put(&format!("/api/channels/{}", channel), &body)
        .await?;
    if json {
        util::print_json(out, &updated)?;
    } else {
        writeln!(
            out,
            "channel '{}' is now {} (poll every {}s)",
            updated.channel,
            if updated.enabled {
                "enabled"
            } else {
                "disabled"
            },
            updated.poll_interval
        )?;
    }
    Ok(())
}

pub async fn run(
    cmd: ChannelsCmd,
    client: &ApiClient,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    match cmd {
        ChannelsCmd::List => {
            let channels: Vec<ChannelConfig> = client.get("/api/channels").await?;
            if json {
                util::print_json(out, &channels)?;
                return Ok(());
            }
            writeln!(
                out,
                "{:<14}  {:<8}  {:>7}  {:<17}  ERROR",
                "CHANNEL", "STATE", "POLL", "LAST POLLED"
            )?;
            for c in &channels {
                print_channel(out, c)?;
            }
            let configured: Vec<&str> = channels.iter().map(|c| c.channel.as_str()).collect();
            let unconfigured: Vec<&str> = crate::collectors::CHANNELS
                .iter()
                .copied()
                .filter(|ch| !configured.contains(ch))
                .collect();
            if !unconfigured.is_empty() {
                writeln!(
                    out,
                    "not yet configured: {} — use `pulp channels enable <channel>`",
                    unconfigured.join(", ")
                )?;
            }
        }
        ChannelsCmd::Get { channel } => {
            let c: ChannelConfig = client.get(&format!("/api/channels/{}", channel)).await?;
            if json {
                util::print_json(out, &c)?;
            } else {
                print_channel(out, &c)?;
                writeln!(out, "    credentials: {}", c.credentials)?;
                writeln!(out, "    max_backfill_days: {}", c.max_backfill_days)?;
            }
        }
        ChannelsCmd::Set {
            channel,
            enabled,
            credentials,
            poll_interval,
        } => {
            set_channel(
                client,
                &channel,
                enabled,
                credentials,
                poll_interval,
                json,
                out,
            )
            .await?
        }
        ChannelsCmd::Enable { channel } => {
            set_channel(client, &channel, Some(true), None, None, json, out).await?
        }
        ChannelsCmd::Disable { channel } => {
            set_channel(client, &channel, Some(false), None, None, json, out).await?
        }
        ChannelsCmd::Cleanup { channel, apply } => {
            let body = CleanupBody { dry_run: !apply };
            let resp: CleanupResponse = client
                .post(&format!("/api/channels/{}/cleanup", channel), &body)
                .await?;
            if json {
                util::print_json(out, &resp)?;
                return Ok(());
            }
            match resp {
                CleanupResponse::Preview(p) => {
                    writeln!(
                        out,
                        "dry run: {} mention(s) would be removed (re-run with --apply to delete)",
                        p.count
                    )?;
                    for s in &p.sample {
                        writeln!(
                            out,
                            "  {}  {}  {}",
                            s.id,
                            s.repo.as_deref().unwrap_or("-"),
                            util::snippet(&s.title, 100)
                        )?;
                    }
                }
                CleanupResponse::Result(r) => {
                    writeln!(out, "deleted {} mention(s)", r.deleted)?;
                }
            }
        }
        ChannelsCmd::Backfill { channel, days } => {
            let body = BackfillBody { days };
            let r: BackfillResult = client
                .post(&format!("/api/channels/{}/backfill", channel), &body)
                .await?;
            if json {
                util::print_json(out, &r)?;
            } else {
                writeln!(out, "{}", r.message)?;
            }
        }
    }
    Ok(())
}
