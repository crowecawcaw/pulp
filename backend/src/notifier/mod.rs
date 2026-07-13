//! Notifier: fan feed-visible mentions out to their workspace's notifications.
//!
//! There is no matching, criteria, or per-monitor toggle. Monitors (and their
//! optional AI filter) already decide what lands in the feed. This worker takes
//! every feed-visible mention that has not yet been notified (`notified_at IS
//! NULL`) and delivers it to ALL notifications in its workspace, then stamps
//! `notified_at` so it never fires again. A mention whose workspace has no
//! notifications is simply marked notified. Fire-once is idempotent via
//! `notified_at`, and adding a notification does NOT replay history (existing
//! mentions are already stamped).

use std::sync::Arc;

use crate::db::repos::traits::{Mention, Notification};
use crate::state::AppState;

pub mod webhook;
pub mod webpush;

/// Mentions fanned out per pass. Bounds one pass; the next resumes where this
/// left off (oldest-first via `list_unnotified`).
const BATCH_SIZE: i64 = 200;

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        run_notifier(state).await;
    });
}

async fn run_notifier(state: Arc<AppState>) {
    tracing::info!("Starting notifier");
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
        run_notify_pass(&state).await;
    }
}

/// Run a single notification pass: for each feed-visible, un-notified mention,
/// deliver to every notification in its workspace, then mark it notified.
/// Public so tests / the admin trigger can drive a pass directly.
pub async fn run_notify_pass(state: &Arc<AppState>) {
    let pending = match state.mentions.list_unnotified(BATCH_SIZE).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Notifier: error listing un-notified mentions: {:?}", e);
            return;
        }
    };
    if pending.is_empty() {
        return;
    }

    // Notifications are looked up once per workspace and reused across that
    // workspace's mentions in the batch.
    let mut cache: std::collections::HashMap<String, Vec<Notification>> =
        std::collections::HashMap::new();
    let mut notified_ids: Vec<String> = Vec::with_capacity(pending.len());

    for item in &pending {
        let notifications = match cache.get(&item.workspace_id) {
            Some(n) => n,
            None => {
                let n = match state
                    .notifications
                    .list_by_workspace(&item.workspace_id)
                    .await
                {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!(
                            "Notifier: error listing notifications for workspace {}: {:?}",
                            item.workspace_id,
                            e
                        );
                        // Skip this mention this pass; retry next pass.
                        continue;
                    }
                };
                cache.entry(item.workspace_id.clone()).or_insert(n)
            }
        };

        for notification in notifications {
            let result = dispatch(state, notification, &item.mention).await;
            if let Err(e) = result {
                tracing::warn!(
                    "Notifier: dispatch failed to {} ({}): {:?}",
                    notification.kind,
                    notification.id,
                    e
                );
            }
        }

        // Mark notified whether or not the workspace had notifications, and even
        // if a delivery failed — the marker is "we attempted the fan-out", so a
        // transient delivery error does not replay forever.
        notified_ids.push(item.mention.id.clone());
    }

    if let Err(e) = state.mentions.mark_notified(&notified_ids).await {
        tracing::error!("Notifier: error marking mentions notified: {:?}", e);
    }
}

/// Deliver one mention to one notification, dispatched by kind.
async fn dispatch(
    state: &Arc<AppState>,
    notification: &Notification,
    mention: &Mention,
) -> anyhow::Result<()> {
    match notification.kind.as_str() {
        "webhook" => {
            let url = notification
                .config
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("webhook notification missing config.url"))?;

            let payload = webhook::format_webhook_payload(mention);
            let resp = state.http.post(url).json(&payload).send().await?;
            if !resp.status().is_success() {
                anyhow::bail!("Webhook returned {}", resp.status());
            }
            Ok(())
        }
        "webpush" => webpush::deliver_to_notification(state, notification, mention).await,
        other => {
            tracing::warn!("Unknown notification kind: {}", other);
            Ok(())
        }
    }
}
