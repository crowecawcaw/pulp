//! `pulp seed` — populate the database with realistic demo data.
//!
//! Unlike the other CLI commands (which are thin HTTP clients over a running
//! server), `seed` opens the SQLite database directly — exactly the way `serve`
//! does (`Config::load` → `create_pool` → run migrations) — because mentions
//! have no public create endpoint (they're written by collectors during
//! ingestion). It uses the server's own repo methods for workspaces, monitors
//! and channels (so those inserts stay compiler-checked against the API
//! contract) and a direct INSERT for the mention rows the repo's
//! ingest-time `insert` can't express (read state, AI verdict).
//!
//! The dataset mirrors the frontend demo fixtures (`frontend/src/demo/`), so the
//! real-backend dev experience and the static demo show the same story. Both
//! draw from the two canonical fictional scenarios documented in
//! `AGENTS.md` ("Test & demo data — canonical scenarios"): **Nimbus Labs**
//! (a company monitoring mentions of its own product) and **Fern** (a solo
//! maintainer watching an open-source project).

use std::io::Write;

use anyhow::Context;
use clap::Args;
use serde_json::json;
use sqlx::SqlitePool;
use ulid::Ulid;

use crate::config::Config;
use crate::db;
use crate::db::repos::traits::{
    ChannelRepo, CreateMonitor, MonitorRepo, NotificationRepo, WorkspaceRepo,
};

/// The demo seeds two workspaces so the UI shows how a multi-project account
/// behaves (the workspace switcher, per-workspace monitors/feed/notifications).
/// Re-running `seed` without `--reset` refuses to create a second copy;
/// `--reset` deletes these workspaces (and their monitors, mentions and
/// notifications) before reseeding.
const WORKSPACE_A: &str = "Nimbus Labs (demo)";
const WORKSPACE_B: &str = "Fern (demo)";
const DEMO_WORKSPACES: [&str; 2] = [WORKSPACE_A, WORKSPACE_B];

#[derive(Args, Debug)]
pub struct SeedArgs {
    /// Delete any existing demo workspace and its data before seeding, so the
    /// command is repeatable.
    #[arg(long)]
    pub reset: bool,
}

pub async fn run(args: SeedArgs, json_out: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    // Open the database the same way `serve` does, honoring PULP_HOME /
    // DATABASE_URL / config.json.
    let config = Config::load()?;
    if let Some(path) = config.database_url.strip_prefix("sqlite:") {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
    }
    let pool = db::pool::create_pool(&config.database_url)
        .await
        .context("opening the database")?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    if args.reset {
        delete_demo_workspaces(&pool).await?;
    } else if demo_workspaces_exist(&pool).await? {
        anyhow::bail!(
            "demo workspaces ({:?}) already exist — re-run with `pulp seed --reset` to \
             replace them",
            DEMO_WORKSPACES
        );
    }

    let summary = seed(&pool, &config.database_url).await?;

    if json_out {
        crate::cli::util::print_json(out, &summary.as_json())?;
    } else {
        writeln!(out, "seeded demo data into {}", config.database_url)?;
        writeln!(
            out,
            "  workspaces: {:?}\n  monitors:  {}\n  mentions:  {}\n  notifications: {}\n  channels enabled: {}",
            DEMO_WORKSPACES,
            summary.monitors,
            summary.mentions,
            summary.notifications,
            summary.channels_enabled,
        )?;
        writeln!(
            out,
            "open the UI (`pulp serve`, then the web app) and switch between the {:?} workspaces.",
            DEMO_WORKSPACES
        )?;
    }
    Ok(())
}

struct Summary {
    monitors: usize,
    mentions: usize,
    notifications: usize,
    channels_enabled: usize,
}

impl Summary {
    fn as_json(&self) -> serde_json::Value {
        json!({
            "workspaces": DEMO_WORKSPACES,
            "monitors": self.monitors,
            "mentions": self.mentions,
            "notifications": self.notifications,
            "channels_enabled": self.channels_enabled,
        })
    }
}

async fn demo_workspaces_exist(pool: &SqlitePool) -> anyhow::Result<bool> {
    let row = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM workspaces WHERE name IN (?, ?)")
        .bind(WORKSPACE_A)
        .bind(WORKSPACE_B)
        .fetch_one(pool)
        .await?;
    Ok(row.0 > 0)
}

/// Remove the demo workspaces and everything under them. Deletes children
/// explicitly (rather than relying on ON DELETE CASCADE) so a reset stays
/// reliable even if that's ever not the case.
async fn delete_demo_workspaces(pool: &SqlitePool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let ws_sel = "SELECT id FROM workspaces WHERE name IN (?, ?)";
    sqlx::query(&format!(
        "DELETE FROM mentions WHERE monitor_id IN \
         (SELECT id FROM monitors WHERE workspace_id IN ({ws_sel}))"
    ))
    .bind(WORKSPACE_A)
    .bind(WORKSPACE_B)
    .execute(&mut *tx)
    .await?;
    for table in ["notifications", "monitors"] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE workspace_id IN ({ws_sel})"
        ))
        .bind(WORKSPACE_A)
        .bind(WORKSPACE_B)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM workspaces WHERE name IN (?, ?)")
        .bind(WORKSPACE_A)
        .bind(WORKSPACE_B)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// A demo mention before it knows its monitor id or absolute timestamps.
struct MentionSeed {
    monitor: MonitorKind,
    channel: &'static str,
    author: &'static str,
    text: &'static str,
    age_hours: i64,
    read: bool,
    ai_verdict: Option<&'static str>,
    platform_meta: serde_json::Value,
}

#[derive(Clone, Copy)]
enum MonitorKind {
    // Workspace A — Nimbus Labs
    Brand,
    Product,
    Competitor,
    // Workspace B — Fern
    FernProject,
    FernBuild,
}

async fn seed(pool: &SqlitePool, _db_url: &str) -> anyhow::Result<Summary> {
    let workspaces = db::repos::workspace::SqliteWorkspaceRepo::new(pool.clone());
    let monitors = db::repos::monitor::SqliteMonitorRepo::new(pool.clone());
    let notifications = db::repos::notification::SqliteNotificationRepo::new(pool.clone());
    let channels = db::repos::channel::SqliteChannelRepo::new(pool.clone());

    // ── Workspace A: Nimbus Labs ─────────────────────────────────────────
    let ws_a = workspaces
        .create(
            WORKSPACE_A,
            Some("Brand, product, and competitor mentions across the developer web"),
        )
        .await?;

    let brand = monitors
        .create(CreateMonitor {
            workspace_id: ws_a.id.clone(),
            terms: vec!["Nimbus".into(), "Nimbus Labs".into()],
            channels: Some(vec!["hackernews".into(), "reddit".into()]),
            exact_match: Some(false),
            case_sensitive: Some(false),
            exclude_terms: Some(vec![]),
            channel_settings: None,
            ai_filter_prompt: None,
        })
        .await?;

    let product = monitors
        .create(CreateMonitor {
            workspace_id: ws_a.id.clone(),
            terms: vec!["nimbusdb".into()],
            channels: Some(vec!["github".into(), "hackernews".into()]),
            exact_match: Some(false),
            case_sensitive: Some(false),
            exclude_terms: Some(vec!["hiring".into(), "job".into()]),
            channel_settings: None,
            ai_filter_prompt: None,
        })
        .await?;

    let competitor = monitors
        .create(CreateMonitor {
            workspace_id: ws_a.id.clone(),
            terms: vec!["Orrery".into()],
            channels: Some(vec!["reddit".into(), "hackernews".into()]),
            exact_match: Some(false),
            case_sensitive: Some(false),
            exclude_terms: Some(vec![]),
            channel_settings: None,
            ai_filter_prompt: Some("Only keep posts comparing the product to ours.".into()),
        })
        .await?;

    // ── Workspace B: Fern (an open-source project a maintainer watches) ──
    let ws_b = workspaces
        .create(
            WORKSPACE_B,
            Some("Buzz, questions, and bug reports about the Fern static-site generator"),
        )
        .await?;

    let fern_project = monitors
        .create(CreateMonitor {
            workspace_id: ws_b.id.clone(),
            terms: vec!["Fern".into(), "fern-ssg".into()],
            channels: Some(vec!["hackernews".into(), "reddit".into()]),
            exact_match: Some(false),
            case_sensitive: Some(false),
            exclude_terms: Some(vec![]),
            channel_settings: None,
            ai_filter_prompt: None,
        })
        .await?;

    // Repo-scoped build/issue watch: `channel_settings` overrides the GitHub
    // collector's `only_repos` for this monitor alone, demonstrating
    // per-monitor channel scoping (see AGENTS.md).
    let fern_build = monitors
        .create(CreateMonitor {
            workspace_id: ws_b.id.clone(),
            terms: vec!["fern".into()],
            channels: Some(vec!["github".into()]),
            exact_match: Some(false),
            case_sensitive: Some(false),
            exclude_terms: Some(vec![]),
            channel_settings: Some(json!({ "github": { "only_repos": ["fern-ssg/fern"] } })),
            ai_filter_prompt: None,
        })
        .await?;

    let monitor_id = |kind: MonitorKind| match kind {
        MonitorKind::Brand => brand.id.clone(),
        MonitorKind::Product => product.id.clone(),
        MonitorKind::Competitor => competitor.id.clone(),
        MonitorKind::FernProject => fern_project.id.clone(),
        MonitorKind::FernBuild => fern_build.id.clone(),
    };

    // Enable all supported channels.
    let enabled = ["hackernews", "reddit", "github"];
    for ch in enabled {
        channels.upsert(ch, true, Some(json!({})), 900).await?;
    }

    // Mentions: enriched rows inserted directly (the repo's ingest-time insert
    // can't set read state).
    let now = chrono::Utc::now().timestamp();
    let mut inserted = 0usize;
    for (i, s) in mention_seeds().iter().enumerate() {
        let published = now - s.age_hours * 3600;
        let id = Ulid::new().to_string();
        let external_id = format!("{}-{}", s.channel, 1000 + i);
        let repo = s.platform_meta.get("repo").and_then(|v| v.as_str());
        let content_url = content_url(s.channel, 1000 + i as i64, repo);
        let meta = serde_json::to_string(&s.platform_meta)?;
        let read_at: Option<i64> = if s.read { Some(published + 600) } else { None };
        let ai_reason = s.ai_verdict.map(|v| {
            if v == "accepted" {
                "Relevant comparison of the product against a competitor"
            } else {
                "Low-quality or off-topic post, not a genuine product discussion"
            }
        });

        sqlx::query(
            "INSERT INTO mentions (id, monitor_id, channel, external_id, content_text, \
             content_url, author_name, author_url, published_at, ingested_at, \
             platform_meta, read_at, ai_verdict, ai_reason) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(monitor_id(s.monitor))
        .bind(s.channel)
        .bind(&external_id)
        .bind(s.text)
        .bind(&content_url)
        .bind(s.author)
        .bind(Option::<String>::None)
        .bind(published)
        .bind(published + 300)
        .bind(&meta)
        .bind(read_at)
        .bind(s.ai_verdict)
        .bind(ai_reason)
        .execute(pool)
        .await?;
        inserted += 1;
    }

    // Notifications: a demo webhook delivery endpoint per workspace. Every feed
    // mention in a workspace fans out to all of its endpoints (no matching).
    let mut notification_count = 0usize;
    for (ws_id, label, url) in [
        (
            &ws_a.id,
            "Ops channel (demo)",
            "https://hooks.example.com/nimbus-labs/demo",
        ),
        (
            &ws_b.id,
            "Maintainer inbox (demo)",
            "https://hooks.example.com/fern/demo",
        ),
    ] {
        notifications
            .create(ws_id, "webhook", &json!({ "url": url }), Some(label))
            .await?;
        notification_count += 1;
    }

    Ok(Summary {
        monitors: 5,
        mentions: inserted,
        notifications: notification_count,
        channels_enabled: enabled.len(),
    })
}

fn content_url(channel: &str, id: i64, repo: Option<&str>) -> String {
    match channel {
        "hackernews" => format!("https://news.ycombinator.com/item?id={id}"),
        "reddit" => format!("https://reddit.com/r/_/comments/{id}"),
        "github" => format!(
            "https://github.com/{}/issues/{id}",
            repo.unwrap_or("nimbus-labs/nimbus")
        ),
        _ => format!("https://example.com/{id}"),
    }
}

fn mention_seeds() -> Vec<MentionSeed> {
    use MonitorKind::*;
    vec![
        // ── Workspace A: Nimbus Labs ──────────────────────────────────────
        MentionSeed {
            monitor: Brand, channel: "hackernews", author: "ops_marlowe",
            text: "Nimbus Labs finally shipped self-hosted mode — this is a big deal for teams that can't send event data to a third party. Tried the Docker compose setup — painless.",
            age_hours: 3,
            read: false, ai_verdict: None,
            platform_meta: json!({
                "kind": "story", "points": 142, "comments": 38,
                "title": "Nimbus ships self-hosted mode",
                "story_url": "https://nimbus.example/blog/self-hosted",
            }),
        },
        MentionSeed {
            monitor: Brand, channel: "reddit", author: "u/dataeng_dan",
            text: "Anyone using Nimbus in production? Considering it over the usual suspects for our event pipeline.",
            age_hours: 9,
            read: false, ai_verdict: None,
            platform_meta: json!({ "subreddit": "dataengineering", "upvotes": 56 }),
        },
        MentionSeed {
            monitor: Brand, channel: "reddit", author: "u/k_hightower",
            text: "The Nimbus SDK ergonomics are genuinely nice. One import, typed events, done.",
            age_hours: 20,
            read: false, ai_verdict: None,
            platform_meta: json!({ "subreddit": "programming", "upvotes": 88 }),
        },
        MentionSeed {
            monitor: Product, channel: "github", author: "kade_rios",
            text: "Repro in the linked gist. Happens consistently when max_connections < 10 under concurrent load.",
            age_hours: 28,
            read: false, ai_verdict: None,
            platform_meta: json!({
                "repo": "nimbus-labs/nimbus", "issue": 2231, "state": "open", "type": "issue",
                "title": "nimbusdb connection pool exhausts under load when max_connections is low",
            }),
        },
        MentionSeed {
            monitor: Product, channel: "github", author: "jdoe",
            text: "The docs cover the primary connection but I can't find anything on replica routing. Using the Go client v0.9.",
            age_hours: 34,
            read: false, ai_verdict: None,
            platform_meta: json!({
                "repo": "nimbus-labs/nimbus-go", "issue": 2240, "state": "open", "type": "issue",
                "title": "How to configure nimbusdb read replicas with the Go client?",
            }),
        },
        MentionSeed {
            monitor: Brand, channel: "hackernews", author: "rachelcodes",
            text: "Replaced our homegrown dashboards-and-scripts setup. Took a weekend. The query editor alone saved us hours a week.",
            age_hours: 40,
            read: true, ai_verdict: None,
            platform_meta: json!({
                "kind": "story", "points": 76, "comments": 18,
                "title": "How I migrated our dashboards to Nimbus in a weekend",
                "story_url": "https://rachelcodes.example/nimbus-migration",
            }),
        },
        MentionSeed {
            monitor: Competitor, channel: "reddit", author: "u/startup_cto",
            text: "We evaluated Orrery vs Nimbus. Orrery was cheaper but the API was painful. Ended up self-hosting Nimbus.",
            age_hours: 74,
            read: false, ai_verdict: Some("accepted"),
            platform_meta: json!({ "subreddit": "startups", "upvotes": 203 }),
        },
        MentionSeed {
            monitor: Competitor, channel: "hackernews", author: "hiring_roundup",
            text: "Orrery is hiring senior engineers — DM me.",
            age_hours: 80,
            read: false, ai_verdict: Some("rejected"),
            platform_meta: json!({
                "kind": "comment", "points": 2, "comments": null,
                "title": "Ask HN: Who is hiring? (June 2026)",
                "story_url": null,
            }),
        },
        MentionSeed {
            monitor: Brand, channel: "reddit", author: "u/pricing_pete",
            text: "Nimbus pricing page is confusing — is the self-hosted license per node or per cluster?",
            age_hours: 96,
            read: false, ai_verdict: None,
            platform_meta: json!({ "subreddit": "selfhosted", "upvotes": 34 }),
        },
        MentionSeed {
            monitor: Brand, channel: "reddit", author: "u/grumpy_sre",
            text: "Nimbus had an outage this morning and the status page was a lie. Not impressed.",
            age_hours: 110,
            read: false, ai_verdict: None,
            platform_meta: json!({ "subreddit": "devops", "upvotes": 77 }),
        },
        MentionSeed {
            monitor: Product, channel: "hackernews", author: "thedevreview",
            text: "Quick tour of nimbusdb's query planner and the new vector index. Timestamps in the description.",
            age_hours: 130,
            read: true, ai_verdict: None,
            platform_meta: json!({
                "kind": "story", "points": 54, "comments": 9,
                "title": "nimbusdb in 100 seconds",
                "story_url": "https://thedevreview.example/nimbusdb-100s",
            }),
        },
        MentionSeed {
            monitor: Brand, channel: "hackernews", author: "dataquill",
            text: "Piped a year of events into a local warehouse in minutes. The filtering options are exactly what I needed.",
            age_hours: 200,
            read: true, ai_verdict: None,
            platform_meta: json!({
                "kind": "comment", "points": 98, "comments": null,
                "title": "Show HN: Nimbus export API",
                "story_url": null,
            }),
        },
        MentionSeed {
            monitor: Product, channel: "hackernews", author: "perf_okonkwo",
            text: "Numbers and methodology inside. Short version: nimbusdb wins on reads, the usual alternatives win on write throughput above 50k TPS.",
            age_hours: 265,
            read: true, ai_verdict: None,
            platform_meta: json!({
                "kind": "story", "points": 210, "comments": 64,
                "title": "Benchmarking nimbusdb vs the obvious alternatives",
                "story_url": "https://perf-okonkwo.example/nimbusdb-benchmark",
            }),
        },
        // ── Workspace B: Fern ─────────────────────────────────────────────
        MentionSeed {
            monitor: FernProject, channel: "hackernews", author: "iris_fern",
            text: "Show HN: Fern, a static-site generator that builds a 2,000-page docs site in under a second.",
            age_hours: 6,
            read: false, ai_verdict: None,
            platform_meta: json!({
                "kind": "story", "points": 117, "comments": 29,
                "title": "Show HN: Fern — a fast static-site generator",
                "story_url": "https://fern.example",
            }),
        },
        MentionSeed {
            monitor: FernProject, channel: "reddit", author: "u/webdev_otto",
            text: "Anyone using Fern for a docs site? Curious how its plugin story compares to what you used before.",
            age_hours: 22,
            read: false, ai_verdict: None,
            platform_meta: json!({ "subreddit": "webdev", "upvotes": 41 }),
        },
        MentionSeed {
            monitor: FernBuild, channel: "github", author: "lena_okoro",
            text: "fern build fails with an out-of-memory error on a 5,000-page site — repro in the attached trace.",
            age_hours: 48,
            read: false, ai_verdict: None,
            platform_meta: json!({
                "repo": "fern-ssg/fern", "issue": 312, "state": "open", "type": "issue",
                "title": "fern build: OOM on large sites",
            }),
        },
        MentionSeed {
            monitor: FernBuild, channel: "github", author: "ci_botley",
            text: "PR: add an fern.config search-index plugin hook requested in issue #298.",
            age_hours: 70,
            read: true, ai_verdict: None,
            platform_meta: json!({
                "repo": "fern-ssg/fern", "issue": 318, "state": "open", "type": "pull",
                "title": "Add search-index plugin hook",
            }),
        },
        MentionSeed {
            monitor: FernProject, channel: "hackernews", author: "deploywatch",
            text: "Show HN: I built a Fern theme gallery so people can see what real sites look like before adopting it.",
            age_hours: 130,
            read: true, ai_verdict: None,
            platform_meta: json!({
                "kind": "story", "points": 88, "comments": 17,
                "title": "Show HN: A theme gallery for Fern",
                "story_url": "https://deploywatch.example/fern-themes",
            }),
        },
        MentionSeed {
            monitor: FernProject, channel: "reddit", author: "u/js_pim",
            text: "Fern vs Bramble — we picked Fern for the simpler config and faster cold builds.",
            age_hours: 190,
            read: true, ai_verdict: None,
            platform_meta: json!({ "subreddit": "javascript", "upvotes": 96 }),
        },
        MentionSeed {
            monitor: FernProject, channel: "reddit", author: "u/linkfarm_lou",
            text: "Best SSGs to build backlinks fast, check my bio for a free guide!!",
            age_hours: 210,
            read: false, ai_verdict: Some("rejected"),
            platform_meta: json!({ "subreddit": "webdev", "upvotes": 1 }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A migrated, shared-cache in-memory database — the same pattern the
    /// integration harness uses so all pool connections see one schema.
    async fn mem_pool() -> SqlitePool {
        let url = format!(
            "sqlite:file:seed-test-{}?mode=memory&cache=shared",
            Ulid::new()
        );
        let pool = db::pool::create_pool(&url).await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    async fn count(pool: &SqlitePool, sql: &str) -> i64 {
        sqlx::query_as::<_, (i64,)>(sql)
            .fetch_one(pool)
            .await
            .unwrap()
            .0
    }

    #[tokio::test]
    async fn seed_populates_enriched_demo_data() {
        let pool = mem_pool().await;
        let summary = seed(&pool, "test").await.unwrap();

        assert_eq!(summary.monitors, 5);
        assert_eq!(summary.mentions, mention_seeds().len());
        assert_eq!(summary.notifications, 2);
        assert_eq!(summary.channels_enabled, 3);

        assert!(demo_workspaces_exist(&pool).await.unwrap());
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM workspaces").await, 2);
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM notifications").await, 2);
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM channel_configs WHERE enabled=1"
            )
            .await,
            3
        );

        // Mentions must be enriched (the repo's ingest-time insert can't set
        // these — this is why seed inserts directly).
        let total = mention_seeds().len() as i64;
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM mentions").await, total);
        // A mix of read/unread and at least one AI verdict each way.
        assert!(
            count(
                &pool,
                "SELECT COUNT(*) FROM mentions WHERE read_at IS NOT NULL"
            )
            .await
                > 0
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM mentions WHERE ai_verdict='accepted'"
            )
            .await,
            1
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM mentions WHERE ai_verdict='rejected'"
            )
            .await,
            2
        );
    }

    #[tokio::test]
    async fn reset_clears_then_reseeds_without_duplicating() {
        let pool = mem_pool().await;
        seed(&pool, "test").await.unwrap();

        delete_demo_workspaces(&pool).await.unwrap();
        assert!(!demo_workspaces_exist(&pool).await.unwrap());
        // Cascade-equivalent cleanup leaves no orphaned children.
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM mentions").await, 0);
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM notifications").await, 0);
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM monitors").await, 0);

        // Re-seeding produces the same counts, not doubled.
        seed(&pool, "test").await.unwrap();
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM workspaces").await, 2);
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM mentions").await,
            mention_seeds().len() as i64
        );
    }
}
