//! End-to-end test of the ingest-time AI filter: pending mentions are hidden
//! from the feed, a filter pass judges them against the monitor's prompt, and
//! only accepted mentions become visible (rejected ones stay queryable).

mod common;

use std::sync::Arc;

use pulp::ai::{AiJudge, AiVerdict};
use pulp::ai_filter;
use pulp::db::repos::traits::NewMention;

/// Accepts texts containing "automation", rejects everything else.
struct KeywordStubJudge;

impl AiJudge for KeywordStubJudge {
    fn judge(&self, _prompt: &str, text: &str) -> Option<AiVerdict> {
        let relevant = text.contains("automation");
        Some(AiVerdict {
            score: if relevant { 1.0 } else { 0.0 },
            reason: Some(if relevant {
                "asks about desktop automation".to_string()
            } else {
                "off-topic".to_string()
            }),
        })
    }
}

fn pending_mention(monitor_id: &str, external_id: &str, text: &str) -> NewMention {
    NewMention {
        monitor_id: monitor_id.to_string(),
        channel: "reddit".to_string(),
        external_id: external_id.to_string(),
        content_text: text.to_string(),
        content_url: format!("https://reddit.com/r/test/{external_id}"),
        author_name: None,
        author_url: None,
        published_at: Some(1_700_000_000),
        platform_meta: serde_json::json!({}),
        ai_verdict: Some("pending".to_string()),
    }
}

#[tokio::test]
async fn ai_filter_pass_gates_the_feed() {
    let app = common::spawn_app_with_ai(Some(Arc::new(KeywordStubJudge))).await;

    // Workspace + monitor with an AI prompt (and per-channel scoping, which
    // must round-trip through the API).
    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": "w" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    let monitor: serde_json::Value = app
        .post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id,
            "terms": ["desktop automation"],
            "channels": ["reddit"],
            "channel_settings": { "reddit": { "subreddits": ["accessibility"] } },
            "ai_filter_prompt": "threads where a desktop automation library is useful"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let monitor_id = monitor["id"].as_str().unwrap();
    assert_eq!(
        monitor["channel_settings"]["reddit"]["subreddits"][0],
        "accessibility"
    );
    assert_eq!(
        monitor["ai_filter_prompt"],
        "threads where a desktop automation library is useful"
    );

    // Two mentions held back for judgment.
    app.state
        .mentions
        .insert(pending_mention(
            monitor_id,
            "t3_keep",
            "looking for a desktop automation tool",
        ))
        .await
        .unwrap();
    app.state
        .mentions
        .insert(pending_mention(
            monitor_id,
            "t3_drop",
            "cheap GPU deals this week",
        ))
        .await
        .unwrap();

    // Before the pass: the feed hides pending mentions.
    let page: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 0);

    ai_filter::run_filter_pass(&app.state).await;

    // After the pass: only the accepted mention is visible by default.
    let page: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["external_id"], "t3_keep");
    assert_eq!(items[0]["ai_verdict"], "accepted");
    assert_eq!(items[0]["ai_reason"], "asks about desktop automation");

    // The rejected mention is kept (soft filter) and reachable explicitly.
    let rejected: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_id}&ai=rejected"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = rejected["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["external_id"], "t3_drop");
    assert_eq!(items[0]["ai_reason"], "off-topic");

    // `ai=all` shows both.
    let all: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_id}&ai=all"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn ai_filter_fails_open_when_ai_disabled() {
    // Mentions queued as pending while AI was enabled must not be stranded
    // when the judge is gone: the pass accepts them with an explanatory reason.
    let app = common::spawn_app().await;

    let ws: serde_json::Value = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": "w" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    let monitor: serde_json::Value = app
        .post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id,
            "terms": ["anything"],
            "ai_filter_prompt": "some prompt"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let monitor_id = monitor["id"].as_str().unwrap();

    app.state
        .mentions
        .insert(pending_mention(monitor_id, "t3_orphan", "stranded item"))
        .await
        .unwrap();

    ai_filter::run_filter_pass(&app.state).await;

    let page: serde_json::Value = app
        .get(&format!("/api/mentions?workspace_id={ws_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["ai_verdict"], "accepted");
}
