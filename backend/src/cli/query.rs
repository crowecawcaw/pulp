//! `pulp query` — search channels live to trial keywords.
//!
//! Runs the server's own `Collector` implementations in-process (no server or
//! database needed), so the results you see here are exactly what a monitor
//! with this phrase would ingest.

use clap::Args;
use serde::Serialize;
use std::io::Write;

use crate::collectors::{self, RawMention};
use crate::db::repos::traits::{Mention, Monitor};

use super::client::ApiClient;
use super::util;

const QUERY_HELP: &str = "\
WORKFLOW (tuning keywords for a monitor):
  1. Start broad and gauge volume/noise:
       pulp query \"desktop automation\" --channel reddit --channel hackernews
  2. Too noisy? Tighten with --exact / --exclude.
  3. Too quiet? Drop --exact, widen --since, or try sibling phrases.
  4. Happy? Save it (subreddit scoping becomes channel settings):
       pulp monitors create \"<phrase>\" --channel <ch> \\
           --channel-settings '{\"reddit\":{\"subreddits\":[\"<sub>\"]}}'

NOTES:
  - Needs no server. Channel credentials are read from a running server's
    config when reachable, else pass --creds '{...}' (reddit and hackernews
    need none).";

#[derive(Args, Debug)]
#[command(after_help = QUERY_HELP)]
pub struct QueryArgs {
    /// The phrase to search for (what a monitor would watch)
    pub phrase: String,

    /// Channel(s) to query; repeat the flag (e.g. --channel reddit)
    #[arg(long = "channel", required = true)]
    pub channels: Vec<String>,

    /// Match the phrase only on word boundaries (monitor exact_match)
    #[arg(long)]
    pub exact: bool,

    /// Case-sensitive matching
    #[arg(long)]
    pub case_sensitive: bool,

    /// Drop items containing this term; repeat the flag (monitor exclude_terms)
    #[arg(long = "exclude")]
    pub exclude: Vec<String>,

    /// How far back to search (7d, 12h, epoch seconds, YYYY-MM-DD)
    #[arg(long, default_value = "7d")]
    pub since: String,

    /// Max items to display per channel
    #[arg(long, default_value_t = 25)]
    pub limit: usize,

    /// Channel credentials JSON or @file.json (default: from the server if
    /// reachable, else {})
    #[arg(long)]
    pub creds: Option<String>,

    /// Reddit only: restrict to subreddit(s); repeat the flag
    #[arg(long = "subreddit")]
    pub subreddits: Vec<String>,
}

/// One fetched item, for --json output.
#[derive(Serialize)]
struct QueryItem {
    mention: Mention,
}

#[derive(Serialize)]
struct ChannelReport {
    channel: String,
    fetched: usize,
    error: Option<String>,
    items: Vec<QueryItem>,
}

/// The ad-hoc monitor `query` simulates; identical semantics to a saved one.
fn ad_hoc_monitor(args: &QueryArgs) -> Monitor {
    let now = chrono::Utc::now().timestamp();
    Monitor {
        id: "query".to_string(),
        workspace_id: "query".to_string(),
        terms: vec![args.phrase.clone()],
        active: true,
        channels: args.channels.clone(),
        exact_match: args.exact,
        case_sensitive: args.case_sensitive,
        exclude_terms: args.exclude.clone(),
        channel_settings: serde_json::json!({}),
        ai_filter_prompt: None,
        created_at: now,
        updated_at: now,
    }
}

/// Lift a collector result into a `Mention` so criteria evaluation sees the
/// same field paths (channel, content_text, platform_meta.*, …) it would on
/// ingested data.
fn raw_to_mention(channel: &str, raw: &RawMention) -> Mention {
    Mention {
        id: raw.external_id.clone(),
        monitor_id: "query".to_string(),
        channel: channel.to_string(),
        external_id: raw.external_id.clone(),
        content_text: raw.content_text.clone(),
        content_url: raw.content_url.clone(),
        author_name: raw.author_name.clone(),
        author_url: raw.author_url.clone(),
        published_at: raw.published_at,
        ingested_at: chrono::Utc::now().timestamp(),
        platform_meta: raw.platform_meta.clone(),
        read_at: None,
        ai_verdict: None,
        ai_reason: None,
    }
}

/// Resolve credentials for a channel: --creds wins; else best-effort from a
/// running server (so saved tokens/subreddit lists apply); else `{}`.
async fn resolve_creds(
    args: &QueryArgs,
    client: &ApiClient,
    channel: &str,
) -> anyhow::Result<serde_json::Value> {
    let mut creds = match args.creds.as_deref() {
        Some(arg) => {
            let text = match arg.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("reading creds file {}: {}", path, e))?,
                None => arg.to_string(),
            };
            serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("--creds is not valid JSON: {}", e))?
        }
        None => client
            .get::<crate::db::repos::traits::ChannelConfig>(&format!("/api/channels/{}", channel))
            .await
            .map(|c| c.credentials)
            .unwrap_or_else(|_| serde_json::json!({})),
    };
    if !creds.is_object() {
        anyhow::bail!("--creds must be a JSON object, e.g. '{{\"user_agent\":\"...\"}}'");
    }
    if channel == "reddit" && !args.subreddits.is_empty() {
        // Same shallow-merge a monitor's channel_settings gets at collection
        // time; --subreddit is CLI sugar for that override.
        let mut overrides = serde_json::json!({ "subreddits": args.subreddits });
        if creds.get("mode").is_none() {
            // Per-subreddit search is the search-mode default once subreddits exist.
            overrides["mode"] = serde_json::json!("search");
        }
        creds = collectors::merged_credentials(&creds, Some(&overrides));
    }
    Ok(creds)
}

pub async fn run(
    args: QueryArgs,
    client: &ApiClient,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    for ch in &args.channels {
        util::ensure_known_channel(ch)?;
    }
    let since = util::parse_since(&args.since)?;
    let monitor = ad_hoc_monitor(&args);
    let http = reqwest::Client::builder()
        .user_agent("Pulp/0.1 (cli query)")
        .build()?;

    let mut reports: Vec<ChannelReport> = Vec::new();
    for channel in &args.channels {
        let collector = collectors::make_collector(channel)
            .expect("channel validated above against collectors::CHANNELS");
        let creds = resolve_creds(&args, client, channel).await?;
        let (raws, error) = match collector.fetch(&monitor, &http, &creds, Some(since)).await {
            Ok(r) => (r, None),
            Err(e) => (vec![], Some(format!("{:#}", e))),
        };
        let mut items: Vec<QueryItem> = raws
            .iter()
            .map(|raw| QueryItem {
                mention: raw_to_mention(channel, raw),
            })
            .collect();
        // Newest first, like the feed.
        items.sort_by_key(|i| std::cmp::Reverse(i.mention.published_at));
        reports.push(ChannelReport {
            channel: channel.clone(),
            fetched: items.len(),
            error,
            items,
        });
    }

    if json {
        for r in &mut reports {
            r.items.truncate(args.limit);
        }
        util::print_json(out, &reports)?;
        return Ok(());
    }

    for r in &reports {
        if let Some(e) = &r.error {
            writeln!(out, "{}: ERROR — {}", r.channel, e)?;
            continue;
        }
        writeln!(
            out,
            "{}: fetched {} since {}",
            r.channel, r.fetched, args.since
        )?;
        for (shown, item) in r.items.iter().enumerate() {
            if shown >= args.limit {
                writeln!(out, "  … ({} more; raise --limit)", r.items.len() - shown)?;
                break;
            }
            let m = &item.mention;
            writeln!(
                out,
                "  {}  {}  {}",
                util::fmt_ts(m.published_at),
                m.author_name.as_deref().unwrap_or("unknown"),
                m.content_url
            )?;
            // For a comment, show the parent thread title for context (the
            // comment body alone doesn't say what it's about). Stories already
            // carry their title at the head of content_text.
            if m.platform_meta.get("kind").and_then(|v| v.as_str()) == Some("comment") {
                if let Some(title) = m.platform_meta.get("title").and_then(|v| v.as_str()) {
                    writeln!(out, "      re: {}", title)?;
                }
            }
            writeln!(out, "      {}", util::snippet(&m.content_text, 160))?;
        }
        if r.fetched == 0 {
            writeln!(
                out,
                "  (nothing found — widen --since, loosen the phrase, or check the channel name)"
            )?;
        }
    }

    Ok(())
}
