use clap::Subcommand;
use std::io::Write;

use crate::db::repos::monitor::split_legacy_phrase;
use crate::db::repos::traits::{CreateMonitor, Monitor, UpdateMonitor};

use super::client::{ApiClient, Qs};
use super::util;

/// Clean up a user-supplied term list before sending it to the API. Each term is
/// meant to be ONE literal phrase, OR-combined with the others. If a single term
/// still carries a top-level ` OR ` (or wrapping quotes), it's auto-split into
/// its component terms via [`split_legacy_phrase`] (the same logic the server's
/// legacy repair uses), with a one-line note on `out` so the user sees it. Blank
/// terms are dropped. Returns the normalized, de-duplicated list.
fn normalize_terms(raw: Vec<String>, out: &mut dyn Write) -> anyhow::Result<Vec<String>> {
    let mut result: Vec<String> = Vec::new();
    for term in raw {
        let split = split_legacy_phrase(&term);
        if split.len() > 1 || (split.len() == 1 && split[0] != term.trim()) {
            writeln!(
                out,
                "note: term {:?} looked like an OR/quoted phrase — split into {:?} \
                 (a term is one literal phrase; terms are OR-combined)",
                term, split
            )?;
        }
        for t in split {
            if !result.contains(&t) {
                result.push(t);
            }
        }
    }
    Ok(result)
}

#[derive(Subcommand, Debug)]
pub enum MonitorsCmd {
    /// List monitors in a workspace
    List {
        /// Workspace id (omit when only one workspace exists)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Show one monitor in full (channel scoping JSON, complete AI prompt)
    Get {
        /// Monitor id
        id: String,
    },
    /// Create a monitor: match ANY of a list of terms, across channels
    #[command(after_help = "EXAMPLES:\n  \
        pulp monitors create \"desktop automation\" --channel reddit --channel hackernews\n  \
        pulp monitors create FlaUI --term Ranorex --term WinAppDriver --channel reddit\n  \
        pulp monitors create \"Pulp\" --exact --exclude \"camera lens\"\n  \
        pulp monitors create \"render farm\" --channel reddit \\\n      \
        --subreddit blender --subreddit vfx \\\n      \
        --ai-prompt \"Is this someone asking for render farm recommendations?\"\n  \
        pulp monitors create \"sdk\" --channel github --only-repo \"my-org/*\"\n\n\
        TERMS: a monitor matches a post if it contains ANY of its terms (they are\n\
        OR-combined). Each term is ONE literal phrase (it may contain spaces);\n\
        pass several with extra positionals or repeated --term flags. Do NOT put\n\
        ` OR ` or quotes inside a term — a term like '\"a\" OR \"b\"' is auto-split\n\
        into two terms with a warning.\n\n\
        --subreddit and --only-repo scope collection per monitor (they are\n\
        shorthand for --channel-settings, which accepts arbitrary per-channel\n\
        JSON merged over that channel's global credentials; monitor keys win).\n\
        --ai-prompt holds new mentions out of the feed until the AI judge\n\
        accepts them (requires AI to be enabled on the server).\n\n\
        Trial terms first with `pulp query <term> --channel <ch>` to check\n\
        the noise level before committing.")]
    Create {
        /// Term(s) to watch — match ANY of them (substring unless --exact).
        /// Each is one literal phrase; pass several positionals and/or --term.
        #[arg(required = true)]
        terms: Vec<String>,
        /// Additional term to watch; repeat the flag (OR-combined with the rest)
        #[arg(long = "term")]
        extra_terms: Vec<String>,
        /// Workspace id (omit when only one workspace exists)
        #[arg(long)]
        workspace: Option<String>,
        /// Channel(s) to watch; repeat the flag (omit = all channels)
        #[arg(long = "channel")]
        channels: Vec<String>,
        /// Match the phrase only on word boundaries
        #[arg(long)]
        exact: bool,
        /// Case-sensitive matching
        #[arg(long)]
        case_sensitive: bool,
        /// Drop items containing this term; repeat the flag
        #[arg(long = "exclude")]
        exclude: Vec<String>,
        /// Per-channel collection overrides as JSON or @file.json, keyed by
        /// channel name (e.g. '{"reddit":{"subreddits":["a11y"]}}')
        #[arg(long)]
        channel_settings: Option<String>,
        /// Scope reddit collection to this subreddit; repeat the flag
        /// (shorthand for --channel-settings '{"reddit":{"subreddits":[…]}}')
        #[arg(long = "subreddit")]
        subreddits: Vec<String>,
        /// Collect only from this github repo (owner/repo, * globs); repeat
        /// the flag (shorthand for '{"github":{"only_repos":[…]}}')
        #[arg(long = "only-repo")]
        only_repos: Vec<String>,
        /// AI relevance prompt: new mentions are held out of the feed until
        /// the AI judge accepts them against this prompt
        #[arg(long)]
        ai_prompt: Option<String>,
    },
    /// Update a monitor (only provided flags change)
    Update {
        /// Monitor id
        id: String,
        /// Replace the term list (match-any); repeat the flag. Each is one
        /// literal phrase; `" OR "`/quotes inside a term are auto-split.
        #[arg(long = "term")]
        terms: Vec<String>,
        /// Replace the channel list; repeat the flag
        #[arg(long = "channel")]
        channels: Option<Vec<String>>,
        /// true/false: match only on word boundaries
        #[arg(long)]
        exact: Option<bool>,
        /// true/false: case-sensitive matching
        #[arg(long)]
        case_sensitive: Option<bool>,
        /// Replace the exclude-term list; repeat the flag
        #[arg(long = "exclude")]
        exclude: Option<Vec<String>>,
        /// true/false: pause (false) or resume (true) collection
        #[arg(long)]
        active: Option<bool>,
        /// Replace per-channel overrides (JSON or @file.json); pass '{}' to clear
        #[arg(long)]
        channel_settings: Option<String>,
        /// Replace the reddit subreddit scoping; repeat the flag (other
        /// channels' settings are kept)
        #[arg(long = "subreddit")]
        subreddits: Vec<String>,
        /// Replace the github only-repos scoping; repeat the flag (other
        /// channels' settings are kept)
        #[arg(long = "only-repo")]
        only_repos: Vec<String>,
        /// Replace the AI relevance prompt; pass an empty string to clear
        #[arg(long)]
        ai_prompt: Option<String>,
    },
    /// Delete a monitor (cascades to its mentions)
    Delete {
        /// Monitor id
        id: String,
    },
}

/// Parse and validate a `--channel-settings` argument: a JSON object keyed by
/// known channel names (each value is merged over that channel's global
/// credentials at collection time).
fn parse_channel_settings(arg: &str) -> anyhow::Result<serde_json::Value> {
    let value = util::load_json(arg, "--channel-settings")?;
    let obj = value.as_object().ok_or_else(|| {
        anyhow::anyhow!(
            "--channel-settings must be a JSON object keyed by channel name, \
             e.g. '{{\"reddit\":{{\"subreddits\":[\"a11y\"]}}}}'"
        )
    })?;
    for key in obj.keys() {
        util::ensure_known_channel(key)?;
    }
    Ok(value)
}

/// Fold the `--subreddit` / `--only-repo` shorthand into a channel-settings
/// object (over `base`, which comes from `--channel-settings` or the
/// monitor's current settings). A non-empty flag list replaces that channel's
/// key; other channels' settings are left alone.
fn apply_scoping_shorthand(
    base: Option<serde_json::Value>,
    subreddits: &[String],
    only_repos: &[String],
) -> Option<serde_json::Value> {
    if subreddits.is_empty() && only_repos.is_empty() {
        return base;
    }
    let mut obj = match base {
        Some(serde_json::Value::Object(o)) => o,
        _ => serde_json::Map::new(),
    };
    let mut set = |channel: &str, key: &str, values: &[String]| {
        if values.is_empty() {
            return;
        }
        let entry = obj
            .entry(channel.to_string())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(ch) = entry.as_object_mut() {
            ch.insert(key.to_string(), serde_json::json!(values));
        }
    };
    set("reddit", "subreddits", subreddits);
    set("github", "only_repos", only_repos);
    Some(serde_json::Value::Object(obj))
}

/// Full single-monitor view: everything `list` truncates (scoping JSON, the
/// complete AI prompt).
fn print_monitor_detail(out: &mut dyn Write, m: &Monitor) -> anyhow::Result<()> {
    writeln!(out, "id:             {}", m.id)?;
    writeln!(out, "workspace:      {}", m.workspace_id)?;
    writeln!(
        out,
        "terms (any of): {}",
        if m.terms.is_empty() {
            "(none — matches nothing)".to_string()
        } else {
            m.terms
                .iter()
                .map(|t| format!("\"{}\"", t))
                .collect::<Vec<_>>()
                .join(", ")
        }
    )?;
    writeln!(
        out,
        "channels:       {}",
        if m.channels.is_empty() {
            "all".to_string()
        } else {
            m.channels.join(", ")
        }
    )?;
    writeln!(
        out,
        "state:          {}",
        if m.active { "active" } else { "paused" }
    )?;
    writeln!(
        out,
        "matching:       {}{}",
        if m.exact_match {
            "exact (word boundaries)"
        } else {
            "substring"
        },
        if m.case_sensitive {
            ", case-sensitive"
        } else {
            ""
        }
    )?;
    if !m.exclude_terms.is_empty() {
        writeln!(out, "exclude terms:  {}", m.exclude_terms.join(", "))?;
    }
    if m.channel_settings
        .as_object()
        .is_some_and(|o| !o.is_empty())
    {
        writeln!(
            out,
            "channel settings:\n{}",
            serde_json::to_string_pretty(&m.channel_settings)?
        )?;
    }
    match &m.ai_filter_prompt {
        Some(p) => writeln!(out, "ai prompt:\n{}", p)?,
        None => writeln!(
            out,
            "ai prompt:      (none — mentions go straight to the feed)"
        )?,
    }
    Ok(())
}

fn print_monitor_row(out: &mut dyn Write, m: &Monitor) -> std::io::Result<()> {
    let channels = if m.channels.is_empty() {
        "all".to_string()
    } else {
        m.channels.join(",")
    };
    let mut flags = Vec::new();
    if m.exact_match {
        flags.push("exact".to_string());
    }
    if m.case_sensitive {
        flags.push("case".to_string());
    }
    if !m.exclude_terms.is_empty() {
        flags.push(format!("exclude:{}", m.exclude_terms.join("|")));
    }
    if m.ai_filter_prompt.is_some() {
        flags.push("ai-filter".to_string());
    }
    if m.channel_settings
        .as_object()
        .is_some_and(|o| !o.is_empty())
    {
        flags.push(format!(
            "scoped:{}",
            m.channel_settings
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>().join("|"))
                .unwrap_or_default()
        ));
    }
    // Match-any term list shown joined, e.g. `"FlaUI" OR "Ranorex"`, truncated.
    let terms_display = if m.terms.is_empty() {
        "(none)".to_string()
    } else {
        m.terms
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    writeln!(
        out,
        "{:<26}  {:<30}  {:<24}  {:<8}  {}",
        m.id,
        util::snippet(&terms_display, 30),
        channels,
        if m.active { "active" } else { "paused" },
        if flags.is_empty() {
            "-".to_string()
        } else {
            flags.join(" ")
        }
    )
}

pub async fn run(
    cmd: MonitorsCmd,
    client: &ApiClient,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    match cmd {
        MonitorsCmd::List { workspace } => {
            let ws = client.resolve_workspace(workspace).await?;
            let mut qs = Qs::new();
            qs.push("workspace_id", &ws);
            let monitors: Vec<Monitor> =
                client.get(&format!("/api/monitors{}", qs.build())).await?;
            if json {
                util::print_json(out, &monitors)?;
            } else if monitors.is_empty() {
                writeln!(
                    out,
                    "no monitors in workspace {} — create one with `pulp monitors create <term>`",
                    ws
                )?;
            } else {
                writeln!(
                    out,
                    "{:<26}  {:<30}  {:<24}  {:<8}  FLAGS",
                    "ID", "TERMS (ANY)", "CHANNELS", "STATE"
                )?;
                for m in &monitors {
                    print_monitor_row(out, m)?;
                }
            }
        }
        MonitorsCmd::Get { id } => {
            let m: Monitor = client.get(&format!("/api/monitors/{}", id)).await?;
            if json {
                util::print_json(out, &m)?;
            } else {
                print_monitor_detail(out, &m)?;
            }
        }
        MonitorsCmd::Create {
            terms,
            extra_terms,
            workspace,
            channels,
            exact,
            case_sensitive,
            exclude,
            channel_settings,
            subreddits,
            only_repos,
            ai_prompt,
        } => {
            for ch in &channels {
                util::ensure_known_channel(ch)?;
            }
            // Positionals + --term, normalized (auto-split stray OR/quotes).
            let all_terms: Vec<String> = terms.into_iter().chain(extra_terms).collect();
            let terms = normalize_terms(all_terms, out)?;
            if terms.is_empty() {
                anyhow::bail!(
                    "no usable terms — a monitor needs at least one literal term to match"
                );
            }
            let settings = channel_settings
                .as_deref()
                .map(parse_channel_settings)
                .transpose()?;
            let settings = apply_scoping_shorthand(settings, &subreddits, &only_repos);
            let ws = client.resolve_workspace(workspace).await?;
            let body = CreateMonitor {
                workspace_id: ws,
                terms,
                channels: if channels.is_empty() {
                    None
                } else {
                    Some(channels)
                },
                exact_match: exact.then_some(true),
                case_sensitive: case_sensitive.then_some(true),
                exclude_terms: if exclude.is_empty() {
                    None
                } else {
                    Some(exclude)
                },
                channel_settings: settings,
                ai_filter_prompt: ai_prompt,
            };
            let m: Monitor = client.post("/api/monitors", &body).await?;
            if json {
                util::print_json(out, &m)?;
            } else {
                writeln!(
                    out,
                    "created monitor (id: {}) matching any of [{}] watching {}",
                    m.id,
                    m.terms
                        .iter()
                        .map(|t| format!("\"{}\"", t))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if m.channels.is_empty() {
                        "all channels".to_string()
                    } else {
                        m.channels.join(", ")
                    }
                )?;
                if m.ai_filter_prompt.is_some() {
                    writeln!(
                        out,
                        "AI filter is on: new mentions stay hidden (ai_verdict=pending) until the \
                         judge accepts them — inspect with `pulp mentions list --ai all`"
                    )?;
                }
                writeln!(
                    out,
                    "collectors poll on their configured interval; run `pulp admin collect <channel>` to fetch now"
                )?;
            }
        }
        MonitorsCmd::Update {
            id,
            terms,
            channels,
            exact,
            case_sensitive,
            exclude,
            active,
            channel_settings,
            subreddits,
            only_repos,
            ai_prompt,
        } => {
            if let Some(chs) = &channels {
                for ch in chs {
                    util::ensure_known_channel(ch)?;
                }
            }
            // No --term flags => leave the term list unchanged; any => replace
            // it with the normalized list (auto-splitting stray OR/quotes).
            let terms = if terms.is_empty() {
                None
            } else {
                let normalized = normalize_terms(terms, out)?;
                if normalized.is_empty() {
                    anyhow::bail!(
                        "--term given but produced no usable terms; pass at least one literal phrase"
                    );
                }
                Some(normalized)
            };
            let mut settings = channel_settings
                .as_deref()
                .map(parse_channel_settings)
                .transpose()?;
            // Shorthand flags without --channel-settings merge into the
            // monitor's CURRENT settings, so e.g. changing the subreddit list
            // doesn't wipe a github scoping.
            if !subreddits.is_empty() || !only_repos.is_empty() {
                let base = match settings.take() {
                    Some(v) => v,
                    None => {
                        let current: Monitor = client.get(&format!("/api/monitors/{}", id)).await?;
                        current.channel_settings
                    }
                };
                settings = apply_scoping_shorthand(Some(base), &subreddits, &only_repos);
            }
            let body = UpdateMonitor {
                terms,
                channels,
                exact_match: exact,
                case_sensitive,
                exclude_terms: exclude,
                active,
                channel_settings: settings,
                ai_filter_prompt: ai_prompt,
            };
            let m: Monitor = client.put(&format!("/api/monitors/{}", id), &body).await?;
            if json {
                util::print_json(out, &m)?;
            } else {
                writeln!(out, "updated monitor {}:", m.id)?;
                print_monitor_row(out, &m)?;
            }
        }
        MonitorsCmd::Delete { id } => {
            client.delete(&format!("/api/monitors/{}", id)).await?;
            writeln!(out, "deleted monitor {}", id)?;
        }
    }
    Ok(())
}
