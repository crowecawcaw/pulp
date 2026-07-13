//! Ingest-time AI filter worker.
//!
//! Mentions collected for a monitor with an `ai_filter_prompt` are inserted
//! with `ai_verdict = 'pending'` and held out of the feed (and SSE stream).
//! This worker drains that queue: it asks the configured [`AiJudge`] whether
//! each mention is relevant per the monitor's prompt and records `accepted`
//! or `rejected` (plus the model's one-sentence reason). Accepted mentions
//! are broadcast to the feed at that point, so the feed only ever shows
//! mentions that passed the filter.
//!
//! Failure policy:
//! - judge not yet *available* (managed model still downloading): skip the
//!   pass entirely and retry later — attempts are not burned;
//! - judge available but a call fails (timeout, unparseable output): bump
//!   `ai_attempts`; after [`MAX_ATTEMPTS`] the mention fails OPEN (accepted
//!   with an explanatory reason) so items are never silently lost;
//! - no judge configured at all (AI disabled after mentions were queued):
//!   accept immediately with an explanatory reason.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::db::repos::traits::Mention;
use crate::state::AppState;

/// How often the worker polls for pending mentions. Judgments themselves take
/// seconds on the local model, so a short interval just bounds idle latency.
const PASS_INTERVAL: Duration = Duration::from_secs(15);

/// Pending mentions judged per pass (oldest first). Keeps one pass bounded;
/// the next pass picks up where this one left off.
const BATCH_SIZE: i64 = 25;

/// Failed judge calls before a mention fails open into the feed.
const MAX_ATTEMPTS: i64 = 5;

/// Scores at or above this are accepted (the judges emit 1.0 / 0.0).
const ACCEPT_THRESHOLD: f64 = 0.5;

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        tracing::info!("Starting AI filter worker");
        loop {
            tokio::time::sleep(PASS_INTERVAL).await;
            run_filter_pass(&state).await;
        }
    });
}

/// Judge one batch of pending mentions. Public so tests / manual triggers can
/// drive a pass directly.
pub async fn run_filter_pass(state: &Arc<AppState>) {
    let pending = match state.mentions.list_ai_pending(BATCH_SIZE).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("AI filter: error listing pending mentions: {:?}", e);
            return;
        }
    };
    if pending.is_empty() {
        return;
    }

    // Re-read the judge each pass so settings changes (enable/disable, new
    // endpoint) take effect on the next pass without a restart.
    let judge = match state.ai_judge() {
        Some(j) => {
            if !j.available() {
                tracing::debug!(
                    "AI filter: judge not available yet; \
                     {} mention(s) stay pending",
                    pending.len()
                );
                return;
            }
            Some(j)
        }
        None => None,
    };

    // Monitors are looked up once per pass; a monitor's prompt applies to all
    // of its mentions in the batch.
    let mut prompts: HashMap<String, Option<String>> = HashMap::new();

    for mention in pending {
        let prompt = match prompts.get(&mention.monitor_id) {
            Some(p) => p.clone(),
            None => {
                let p = match state.monitors.get(&mention.monitor_id).await {
                    Ok(m) => m
                        .and_then(|m| m.ai_filter_prompt)
                        .filter(|p| !p.trim().is_empty()),
                    Err(e) => {
                        tracing::error!(
                            "AI filter: error loading monitor {}: {:?}",
                            mention.monitor_id,
                            e
                        );
                        continue;
                    }
                };
                prompts.insert(mention.monitor_id.clone(), p.clone());
                p
            }
        };

        let (judge, prompt) = match (&judge, prompt) {
            (Some(j), Some(p)) => (j.clone(), p),
            // AI disabled, or the prompt was removed since ingest: there is
            // nothing to judge against — let the mention through.
            (None, _) => {
                accept(
                    state,
                    &mention,
                    Some("AI filtering is disabled; shown unfiltered"),
                )
                .await;
                continue;
            }
            (_, None) => {
                accept(state, &mention, None).await;
                continue;
            }
        };

        // The judge call blocks for seconds (local inference); keep it off
        // the async worker threads.
        let text = mention.content_text.clone();
        let verdict = tokio::task::spawn_blocking(move || judge.judge(&prompt, &text))
            .await
            .ok()
            .flatten();

        match verdict {
            Some(v) if v.score >= ACCEPT_THRESHOLD => {
                set_verdict(state, &mention, "accepted", v.reason.as_deref(), true).await;
            }
            Some(v) => {
                set_verdict(state, &mention, "rejected", v.reason.as_deref(), false).await;
            }
            None => {
                let attempts = match state.mentions.bump_ai_attempts(&mention.id).await {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!(
                            "AI filter: error bumping attempts for {}: {:?}",
                            mention.id,
                            e
                        );
                        continue;
                    }
                };
                if attempts >= MAX_ATTEMPTS {
                    tracing::warn!(
                        "AI filter: mention {} unjudgeable after {} attempts; failing open",
                        mention.id,
                        attempts
                    );
                    accept(
                        state,
                        &mention,
                        Some("AI filter could not judge this mention; shown unfiltered"),
                    )
                    .await;
                }
            }
        }
    }
}

async fn accept(state: &Arc<AppState>, mention: &Mention, reason: Option<&str>) {
    set_verdict(state, mention, "accepted", reason, true).await;
}

/// Record the verdict; accepted mentions are broadcast to the feed now (they
/// were held back at insert time).
async fn set_verdict(
    state: &Arc<AppState>,
    mention: &Mention,
    verdict: &str,
    reason: Option<&str>,
    broadcast: bool,
) {
    match state
        .mentions
        .set_ai_verdict(&mention.id, verdict, reason)
        .await
    {
        Ok(updated) => {
            tracing::info!(
                "AI filter: mention {} {} ({})",
                updated.id,
                verdict,
                reason.unwrap_or("no reason")
            );
            if broadcast {
                if let Ok(json) = serde_json::to_string(&updated) {
                    let _ = state.sse_tx.send(json);
                }
            }
        }
        Err(e) => {
            tracing::error!(
                "AI filter: error recording verdict for {}: {:?}",
                mention.id,
                e
            );
        }
    }
}
