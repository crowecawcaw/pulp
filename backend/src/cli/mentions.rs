use clap::Subcommand;
use std::io::Write;

use crate::api::mentions::{MentionPage, SetReadRequest};
use crate::db::repos::traits::Mention;

use super::client::{ApiClient, Qs};
use super::util;

#[derive(Subcommand, Debug)]
pub enum MentionsCmd {
    /// List ingested mentions, newest first (the feed)
    #[command(after_help = "EXAMPLES:\n  \
        pulp mentions list --since 1d --unread\n  \
        pulp mentions list --channel reddit --limit 50\n  \
        pulp mentions list --before 1765300000        # page further back\n\n\
        Pagination is keyset-based: when output ends with 'more available', pass the\n\
        oldest shown published_at as --before to get the next page.")]
    List {
        /// Filter to one workspace
        #[arg(long)]
        workspace: Option<String>,
        /// Filter to one channel (e.g. reddit)
        #[arg(long)]
        channel: Option<String>,
        /// Filter to one monitor id
        #[arg(long)]
        monitor: Option<String>,
        /// Only mentions published after this point (7d, 12h, epoch, YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,
        /// Keyset cursor: only mentions published strictly before this epoch
        #[arg(long)]
        before: Option<i64>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
        /// Only unread mentions
        #[arg(long, conflicts_with = "read")]
        unread: bool,
        /// Only read mentions
        #[arg(long)]
        read: bool,
        /// AI-filter view: default shows feed-visible mentions (no verdict or
        /// accepted); `all` shows everything; or pick one verdict
        #[arg(long, value_parser = ["visible", "all", "pending", "accepted", "rejected"])]
        ai: Option<String>,
    },
    /// Mark mention(s) read
    MarkRead {
        /// Mention id(s)
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Mark mention(s) unread
    MarkUnread {
        /// Mention id(s)
        #[arg(required = true)]
        ids: Vec<String>,
    },
}

pub fn print_mention(out: &mut dyn Write, m: &Mention) -> std::io::Result<()> {
    let ai_marker = match m.ai_verdict.as_deref() {
        // `accepted` is the normal feed state — not worth a marker.
        Some(v @ ("pending" | "rejected")) => format!("  [ai:{}]", v),
        _ => String::new(),
    };
    writeln!(
        out,
        "{}  [{}]  {}  {}{}{}",
        m.id,
        m.channel,
        util::fmt_ts(m.published_at),
        m.author_name.as_deref().unwrap_or("unknown"),
        if m.read_at.is_some() {
            ""
        } else {
            "  (unread)"
        },
        ai_marker
    )?;
    writeln!(out, "    {}", util::snippet(&m.content_text, 160))?;
    if let Some(reason) = &m.ai_reason {
        writeln!(
            out,
            "    ai {}: {}",
            m.ai_verdict.as_deref().unwrap_or("judged"),
            util::snippet(reason, 140)
        )?;
    }
    writeln!(out, "    {}", m.content_url)
}

async fn set_read(
    client: &ApiClient,
    ids: Vec<String>,
    read: bool,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let mut updated = Vec::new();
    for id in ids {
        let m: Mention = client
            .put(
                &format!("/api/mentions/{}/read", id),
                &SetReadRequest { read },
            )
            .await?;
        updated.push(m);
    }
    if json {
        util::print_json(out, &updated)?;
    } else {
        writeln!(
            out,
            "marked {} mention(s) {}",
            updated.len(),
            if read { "read" } else { "unread" }
        )?;
    }
    Ok(())
}

pub async fn run(
    cmd: MentionsCmd,
    client: &ApiClient,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    match cmd {
        MentionsCmd::List {
            workspace,
            channel,
            monitor,
            since,
            before,
            limit,
            unread,
            read,
            ai,
        } => {
            let since_ts = since.as_deref().map(util::parse_since).transpose()?;
            let mut qs = Qs::new();
            qs.push_opt("workspace_id", workspace)
                .push_opt("channel", channel)
                .push_opt("monitor_id", monitor)
                .push_opt("since", since_ts)
                .push_opt("before", before)
                .push_opt("ai", ai)
                .push("limit", limit);
            if unread {
                qs.push("read", false);
            } else if read {
                qs.push("read", true);
            }
            let page: MentionPage = client.get(&format!("/api/mentions{}", qs.build())).await?;
            if json {
                util::print_json(out, &page)?;
                return Ok(());
            }
            if page.items.is_empty() {
                writeln!(
                    out,
                    "no mentions match — collectors may not have run yet (`pulp admin collect <channel>`), \
                     mentions may be held by an AI filter (try --ai all), or widen the filters (--since, --limit)"
                )?;
                return Ok(());
            }
            for m in &page.items {
                print_mention(out, m)?;
            }
            write!(out, "{} mention(s) shown", page.items.len())?;
            if page.has_more {
                let oldest = page.items.iter().filter_map(|m| m.published_at).min();
                match oldest {
                    Some(ts) => writeln!(out, "; more available — pass --before {}", ts)?,
                    None => writeln!(out, "; more available — raise --limit")?,
                }
            } else {
                writeln!(out)?;
            }
        }
        MentionsCmd::MarkRead { ids } => set_read(client, ids, true, json, out).await?,
        MentionsCmd::MarkUnread { ids } => set_read(client, ids, false, json, out).await?,
    }
    Ok(())
}
