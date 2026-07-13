use clap::Subcommand;
use std::io::Write;

use crate::db::repos::traits::{CreateNotification, Notification};

use super::client::{ApiClient, Qs};
use super::util;

pub const LONG_ABOUT: &str = "\
Manage a workspace's notifications. A notification is a pure delivery endpoint:
every mention that enters the feed fans out to ALL of its workspace's
notifications — there is no matching, criteria, or per-monitor toggle.

Kinds:
  webhook  a plain URL POST (add it here with add-webhook)
  webpush  a browser/PWA Web Push subscription (added from the browser, not the
           CLI — open the app and enable notifications on the device)

EXAMPLES:
  pulp notifications list
  pulp notifications add-webhook --url https://example.com/hook --label slack
  pulp notifications test                 # send a test to every notification
  pulp notifications remove <id>";

#[derive(Subcommand, Debug)]
pub enum NotificationsCmd {
    /// List a workspace's notifications
    List {
        /// Workspace id (omit when only one workspace exists)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Add a webhook notification (POSTs each feed mention to a URL)
    AddWebhook {
        /// The URL to POST mentions to
        #[arg(long)]
        url: String,
        /// Workspace id (omit when only one workspace exists)
        #[arg(long)]
        workspace: Option<String>,
        /// Optional human label
        #[arg(long)]
        label: Option<String>,
    },
    /// Remove a notification by id
    Remove {
        /// Notification id
        id: String,
    },
    /// Send a test notification to every notification in the workspace
    Test {
        /// Workspace id (omit when only one workspace exists)
        #[arg(long)]
        workspace: Option<String>,
    },
}

fn print_notification(out: &mut dyn Write, n: &Notification) -> std::io::Result<()> {
    let detail = match n.kind.as_str() {
        "webhook" => n
            .config
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("(no url)")
            .to_string(),
        "webpush" => n
            .config
            .get("endpoint")
            .and_then(|v| v.as_str())
            .map(|e| {
                e.split_once("://")
                    .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
                    .unwrap_or(e)
                    .to_string()
            })
            .unwrap_or_else(|| "(no endpoint)".to_string()),
        _ => util::snippet(&n.config.to_string(), 60),
    };
    writeln!(
        out,
        "{}  {:<8}  {:<20}  {}",
        n.id,
        n.kind,
        n.label.as_deref().unwrap_or("-"),
        detail
    )
}

pub async fn run(
    cmd: NotificationsCmd,
    client: &ApiClient,
    json: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    match cmd {
        NotificationsCmd::List { workspace } => {
            let ws = client.resolve_workspace(workspace).await?;
            let mut qs = Qs::new();
            qs.push("workspace_id", &ws);
            let items: Vec<Notification> = client
                .get(&format!("/api/notifications{}", qs.build()))
                .await?;
            if json {
                util::print_json(out, &items)?;
            } else if items.is_empty() {
                writeln!(
                    out,
                    "no notifications in workspace {} — add one with \
                     `pulp notifications add-webhook --url <url>`",
                    ws
                )?;
            } else {
                for n in &items {
                    print_notification(out, n)?;
                }
            }
        }
        NotificationsCmd::AddWebhook {
            url,
            workspace,
            label,
        } => {
            let ws = client.resolve_workspace(workspace).await?;
            let body = CreateNotification {
                workspace_id: ws,
                kind: "webhook".into(),
                config: serde_json::json!({ "url": url }),
                label,
            };
            let n: Notification = client.post("/api/notifications", &body).await?;
            if json {
                util::print_json(out, &n)?;
            } else {
                writeln!(out, "added webhook notification (id: {})", n.id)?;
            }
        }
        NotificationsCmd::Remove { id } => {
            client.delete(&format!("/api/notifications/{}", id)).await?;
            writeln!(out, "removed notification {}", id)?;
        }
        NotificationsCmd::Test { workspace } => {
            let ws = client.resolve_workspace(workspace).await?;
            let mut qs = Qs::new();
            qs.push("workspace_id", &ws);
            let result: super::super::api::notifications::TestNotificationResult = client
                .post_query(&format!("/api/notifications/test{}", qs.build()))
                .await?;
            if json {
                util::print_json(out, &serde_json::json!({ "delivered": result.delivered }))?;
            } else {
                writeln!(
                    out,
                    "test notification attempted on {} notification(s) in workspace {}",
                    result.delivered, ws
                )?;
            }
        }
    }
    Ok(())
}
