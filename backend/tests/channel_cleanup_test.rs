mod common;

use pulp::db::repos::traits::NewMention;

#[tokio::test]
async fn cleanup_dry_run_returns_count_of_matching_mentions() {
    let app = common::spawn_app().await;

    // Create workspace
    let ws_resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "test"}))
        .send()
        .await
        .unwrap();
    let ws: serde_json::Value = ws_resp.json().await.unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    // Create monitor
    let kw_resp = app
        .post("/api/monitors")
        .json(&serde_json::json!({"workspace_id": ws_id, "terms": ["accessibility"]}))
        .send()
        .await
        .unwrap();
    let kw: serde_json::Value = kw_resp.json().await.unwrap();
    let kw_id = kw["id"].as_str().unwrap().to_string();

    // Configure github channel with ignore_orgs: ["noisy-org"]
    app.put("/api/channels/github")
        .json(&serde_json::json!({
            "enabled": true,
            "credentials": { "token": "", "ignore_orgs": ["noisy-org"] }
        }))
        .send()
        .await
        .unwrap();

    // Insert two mentions: one from noisy-org (should be cleaned), one external (kept)
    let noisy_meta =
        serde_json::json!({"repo": "noisy-org/their-repo", "state": "open", "type": "issue"});
    let clean_meta =
        serde_json::json!({"repo": "external-org/good-repo", "state": "open", "type": "issue"});

    app.state
        .mentions
        .insert(NewMention {
            monitor_id: kw_id.clone(),
            channel: "github".to_string(),
            external_id: "gh_1001".to_string(),
            content_text: "Accessibility issue in noisy-org".to_string(),
            content_url: "https://github.com/noisy-org/their-repo/issues/1".to_string(),
            author_name: Some("alice".to_string()),
            author_url: None,
            published_at: None,
            platform_meta: noisy_meta,
            ai_verdict: None,
        })
        .await
        .unwrap();

    app.state
        .mentions
        .insert(NewMention {
            monitor_id: kw_id.clone(),
            channel: "github".to_string(),
            external_id: "gh_1002".to_string(),
            content_text: "Accessibility issue in external-org".to_string(),
            content_url: "https://github.com/external-org/good-repo/issues/2".to_string(),
            author_name: Some("bob".to_string()),
            author_url: None,
            published_at: None,
            platform_meta: clean_meta,
            ai_verdict: None,
        })
        .await
        .unwrap();

    // Dry run
    let resp = app
        .post("/api/channels/github/cleanup")
        .json(&serde_json::json!({"dry_run": true}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["count"].as_u64().unwrap(), 1);
    // sample should contain the noisy-org mention
    let sample = body["sample"].as_array().unwrap();
    assert_eq!(sample.len(), 1);
    assert_eq!(sample[0]["repo"].as_str().unwrap(), "noisy-org/their-repo");

    // The mentions should still be in the DB (dry run doesn't delete)
    let count: i64 =
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM mentions WHERE channel = 'github'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
            .0;
    assert_eq!(count, 2);
}

#[tokio::test]
async fn cleanup_deletes_all_noisy_mentions_but_preserves_good_ones() {
    let app = common::spawn_app().await;

    // Create workspace
    let ws_resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({"name": "test"}))
        .send()
        .await
        .unwrap();
    let ws: serde_json::Value = ws_resp.json().await.unwrap();
    let ws_id = ws["id"].as_str().unwrap();

    // Create monitor
    let kw_resp = app
        .post("/api/monitors")
        .json(&serde_json::json!({"workspace_id": ws_id, "terms": ["accessibility"]}))
        .send()
        .await
        .unwrap();
    let kw: serde_json::Value = kw_resp.json().await.unwrap();
    let kw_id = kw["id"].as_str().unwrap().to_string();

    app.put("/api/channels/github")
        .json(&serde_json::json!({
            "enabled": true,
            "credentials": { "token": "", "ignore_repos": ["spam-org/*"] }
        }))
        .send()
        .await
        .unwrap();

    let spam_meta =
        serde_json::json!({"repo": "spam-org/spam-repo", "state": "open", "type": "issue"});
    let spam_meta_2 =
        serde_json::json!({"repo": "spam-org/also-spam", "state": "open", "type": "issue"});
    let good_meta =
        serde_json::json!({"repo": "good-org/good-repo", "state": "open", "type": "issue"});

    // Insert a spam mention (should be deleted)
    app.state
        .mentions
        .insert(NewMention {
            monitor_id: kw_id.clone(),
            channel: "github".to_string(),
            external_id: "gh_2001".to_string(),
            content_text: "spam pending".to_string(),
            content_url: "https://github.com/spam-org/spam-repo/issues/1".to_string(),
            author_name: None,
            author_url: None,
            published_at: None,
            platform_meta: spam_meta,
            ai_verdict: None,
        })
        .await
        .unwrap();

    // Insert a second spam mention from the same ignored org (should also be
    // deleted — there is no "archived" exemption; mentions have no
    // ingestion/scoring lifecycle status to preserve them).
    app.state
        .mentions
        .insert(NewMention {
            monitor_id: kw_id.clone(),
            channel: "github".to_string(),
            external_id: "gh_2002".to_string(),
            content_text: "spam but actioned".to_string(),
            content_url: "https://github.com/spam-org/also-spam/issues/2".to_string(),
            author_name: None,
            author_url: None,
            published_at: None,
            platform_meta: spam_meta_2,
            ai_verdict: None,
        })
        .await
        .unwrap();

    // Insert good mention (should NOT be deleted)
    app.state
        .mentions
        .insert(NewMention {
            monitor_id: kw_id.clone(),
            channel: "github".to_string(),
            external_id: "gh_2003".to_string(),
            content_text: "good mention".to_string(),
            content_url: "https://github.com/good-org/good-repo/issues/3".to_string(),
            author_name: None,
            author_url: None,
            published_at: None,
            platform_meta: good_meta,
            ai_verdict: None,
        })
        .await
        .unwrap();

    // Actual cleanup (not dry run)
    let resp = app
        .post("/api/channels/github/cleanup")
        .json(&serde_json::json!({"dry_run": false}))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["deleted"].as_u64().unwrap(), 2);

    // Only the good mention should remain.
    let all_remaining: i64 =
        sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM mentions WHERE channel = 'github'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap()
            .0;
    assert_eq!(all_remaining, 1);
}
