//! End-to-end CLI tests: drive `pulp::cli::execute` against a real
//! in-process server (the same `spawn_app` the API tests use), capturing
//! stdout. This exercises the full path the binary takes minus process spawn:
//! clap parse -> HTTP client -> Axum handlers -> SQLite.

mod common;

use clap::Parser;
use pulp::cli::{execute, Cli};
use pulp::db::repos::traits::NewMention;
use serial_test::serial;

/// Run one CLI invocation against the test server, returning captured stdout.
async fn cli(app: &common::TestApp, args: &[&str]) -> anyhow::Result<String> {
    let mut argv: Vec<String> = vec![
        "pulp".to_string(),
        "--server".to_string(),
        app.base_url.clone(),
    ];
    argv.extend(args.iter().map(|s| s.to_string()));
    let parsed = Cli::try_parse_from(argv).expect("CLI args should parse");
    let mut out: Vec<u8> = Vec::new();
    execute(parsed, &mut out).await?;
    Ok(String::from_utf8(out).expect("CLI output is UTF-8"))
}

async fn insert_mention(app: &common::TestApp, monitor_id: &str, text: &str, external_id: &str) {
    insert_mention_with_verdict(app, monitor_id, text, external_id, None).await;
}

async fn insert_mention_with_verdict(
    app: &common::TestApp,
    monitor_id: &str,
    text: &str,
    external_id: &str,
    ai_verdict: Option<&str>,
) {
    app.state
        .mentions
        .insert(NewMention {
            monitor_id: monitor_id.to_string(),
            channel: "reddit".to_string(),
            external_id: external_id.to_string(),
            content_text: text.to_string(),
            content_url: format!("https://reddit.com/{}", external_id),
            author_name: Some("tester".to_string()),
            author_url: None,
            published_at: Some(chrono::Utc::now().timestamp() - 3_600),
            platform_meta: serde_json::json!({ "subreddit": "testing" }),
            ai_verdict: ai_verdict.map(String::from),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn config_lists_gets_and_sets_ai_filter() {
    let app = common::spawn_app().await;

    // Bare `config` lists every key with its current value and default.
    let out = cli(&app, &["config"]).await.unwrap();
    for key in ["enabled", "base_url", "model", "api_key"] {
        assert!(out.contains(key), "list should mention {key}; out: {out}");
    }
    assert!(
        out.contains("(unset)"),
        "model defaults to unset; out: {out}"
    );

    // get reflects defaults: model unset, api_key never leaks its value.
    let out = cli(&app, &["config", "get", "model"]).await.unwrap();
    assert_eq!(out.trim(), "(unset)");

    // set persists + is readable back through get.
    let out = cli(&app, &["config", "set", "model", "llama3.2"])
        .await
        .unwrap();
    assert!(out.contains("llama3.2"), "out: {out}");
    let out = cli(&app, &["config", "get", "model"]).await.unwrap();
    assert_eq!(out.trim(), "llama3.2");

    // enabling needs a model (set above) and base_url (default is non-empty).
    let out = cli(&app, &["config", "set", "enabled", "true"])
        .await
        .unwrap();
    assert!(out.contains("enabled:  true"), "out: {out}");

    // api_key get reports only whether one is set, never the secret.
    cli(&app, &["config", "set", "api_key", "sk-secret-123"])
        .await
        .unwrap();
    let out = cli(&app, &["config", "get", "api_key"]).await.unwrap();
    assert_eq!(out.trim(), "set");
    assert!(
        !out.contains("sk-secret-123"),
        "secret must not leak: {out}"
    );

    // --json list is structured with current + defaults.
    let out = cli(&app, &["--json", "config"]).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["current"]["model"], "llama3.2");
    assert_eq!(v["current"]["enabled"], true);
    assert_eq!(v["current"]["api_key_set"], true);
    assert_eq!(v["defaults"]["enabled"], false);

    // Unknown keys are rejected with the valid list.
    let err = cli(&app, &["config", "get", "temperature"])
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("temperature") && err.contains("base_url"),
        "err: {err}"
    );
}

#[tokio::test]
async fn workspace_and_monitor_lifecycle() {
    let app = common::spawn_app().await;

    let out = cli(&app, &["workspaces", "create", "CLI WS"])
        .await
        .unwrap();
    assert!(out.contains("created workspace 'CLI WS'"), "out: {out}");

    // --json output round-trips through the shared DTO structs.
    let out = cli(&app, &["--json", "workspaces", "list"]).await.unwrap();
    let ws: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(ws.as_array().unwrap().len(), 1);

    // Workspace is auto-resolved when only one exists.
    let out = cli(
        &app,
        &[
            "monitors",
            "create",
            "desktop automation",
            "--term",
            "pywinauto",
            "--channel",
            "reddit",
            "--exclude",
            "hiring",
        ],
    )
    .await
    .unwrap();
    assert!(
        out.contains("\"desktop automation\"") && out.contains("\"pywinauto\""),
        "out: {out}"
    );

    let out = cli(&app, &["--json", "monitors", "list"]).await.unwrap();
    let monitors: serde_json::Value = serde_json::from_str(&out).unwrap();
    let m = &monitors.as_array().unwrap()[0];
    assert_eq!(m["terms"][0], "desktop automation");
    assert_eq!(m["terms"][1], "pywinauto");
    assert_eq!(m["channels"][0], "reddit");
    assert_eq!(m["exclude_terms"][0], "hiring");
    let monitor_id = m["id"].as_str().unwrap().to_string();

    let out = cli(
        &app,
        &["monitors", "update", &monitor_id, "--active", "false"],
    )
    .await
    .unwrap();
    assert!(out.contains("paused"), "out: {out}");

    cli(&app, &["monitors", "delete", &monitor_id])
        .await
        .unwrap();
    let out = cli(&app, &["monitors", "list"]).await.unwrap();
    assert!(out.contains("no monitors"), "out: {out}");

    // Unknown channels are rejected client-side with the valid list.
    let err = cli(&app, &["monitors", "create", "x", "--channel", "twitter"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown channel"), "err: {err}");
}

#[tokio::test]
async fn mentions_feed_and_read_state() {
    let app = common::spawn_app().await;
    cli(&app, &["workspaces", "create", "WS"]).await.unwrap();
    let out = cli(&app, &["--json", "monitors", "create", "testbrand"])
        .await
        .unwrap();
    let monitor: serde_json::Value = serde_json::from_str(&out).unwrap();
    let monitor_id = monitor["id"].as_str().unwrap();

    insert_mention(
        &app,
        monitor_id,
        "testbrand is great for monitoring",
        "t3_one",
    )
    .await;

    let out = cli(&app, &["mentions", "list", "--since", "1d"])
        .await
        .unwrap();
    assert!(out.contains("testbrand is great"), "out: {out}");
    assert!(out.contains("(unread)"), "out: {out}");

    let out = cli(&app, &["--json", "mentions", "list"]).await.unwrap();
    let page: serde_json::Value = serde_json::from_str(&out).unwrap();
    let id = page["items"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(page["has_more"], false);

    let out = cli(&app, &["mentions", "mark-read", &id]).await.unwrap();
    assert!(out.contains("marked 1 mention(s) read"), "out: {out}");

    let out = cli(&app, &["mentions", "list", "--unread"]).await.unwrap();
    assert!(out.contains("no mentions match"), "out: {out}");
}

#[tokio::test]
async fn channels_set_preserves_unset_fields() {
    let app = common::spawn_app().await;

    cli(
        &app,
        &[
            "channels",
            "set",
            "reddit",
            "--credentials",
            r#"{"subreddits":["QualityAssurance"]}"#,
            "--poll-interval",
            "600",
        ],
    )
    .await
    .unwrap();

    // Enabling later must not wipe the saved credentials (the raw API would).
    let out = cli(&app, &["channels", "enable", "reddit"]).await.unwrap();
    assert!(out.contains("now enabled"), "out: {out}");

    let out = cli(&app, &["--json", "channels", "get", "reddit"])
        .await
        .unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(cfg["enabled"], true);
    assert_eq!(cfg["poll_interval"], 600);
    assert_eq!(cfg["credentials"]["subreddits"][0], "QualityAssurance");

    // Migrations seed a row for every channel, so the list shows them all.
    let out = cli(&app, &["channels", "list"]).await.unwrap();
    assert!(out.contains("reddit          enabled"), "out: {out}");
    assert!(out.contains("hackernews      disabled"), "out: {out}");
}

#[tokio::test]
async fn notifications_lifecycle() {
    let app = common::spawn_app().await;
    cli(&app, &["workspaces", "create", "WS"]).await.unwrap();

    let out = cli(
        &app,
        &[
            "notifications",
            "add-webhook",
            "--url",
            "https://example.com/hook",
            "--label",
            "slack",
        ],
    )
    .await
    .unwrap();
    assert!(out.contains("added webhook notification"), "out: {out}");

    let out = cli(&app, &["--json", "notifications", "list"])
        .await
        .unwrap();
    let items: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(items[0]["kind"], "webhook");
    assert_eq!(items[0]["config"]["url"], "https://example.com/hook");
    assert_eq!(items[0]["label"], "slack");
    let id = items[0]["id"].as_str().unwrap().to_string();

    // Remove it.
    let out = cli(&app, &["notifications", "remove", &id]).await.unwrap();
    assert!(out.contains("removed notification"), "out: {out}");

    let out = cli(&app, &["--json", "notifications", "list"])
        .await
        .unwrap();
    let items: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(items.as_array().unwrap().is_empty(), "out: {out}");
}

#[tokio::test]
async fn monitor_channel_settings_and_ai_prompt() {
    let app = common::spawn_app().await;
    cli(&app, &["workspaces", "create", "WS"]).await.unwrap();

    let out = cli(
        &app,
        &[
            "--json",
            "monitors",
            "create",
            "a11y",
            "--channel",
            "reddit",
            "--channel-settings",
            r#"{"reddit":{"subreddits":["accessibility"]}}"#,
            "--ai-prompt",
            "Is this about software accessibility?",
        ],
    )
    .await
    .unwrap();
    let m: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        m["channel_settings"]["reddit"]["subreddits"][0],
        "accessibility"
    );
    assert_eq!(
        m["ai_filter_prompt"],
        "Is this about software accessibility?"
    );
    let id = m["id"].as_str().unwrap().to_string();

    // The human listing surfaces both as flags.
    let out = cli(&app, &["monitors", "list"]).await.unwrap();
    assert!(out.contains("ai-filter"), "out: {out}");
    assert!(out.contains("scoped:reddit"), "out: {out}");

    // Clearing the prompt via update (empty string clears, per the API).
    let out = cli(
        &app,
        &["--json", "monitors", "update", &id, "--ai-prompt", ""],
    )
    .await
    .unwrap();
    let m: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(m["ai_filter_prompt"].is_null(), "m: {m}");

    // Settings keyed by an unknown channel are rejected client-side.
    let err = cli(
        &app,
        &[
            "monitors",
            "create",
            "x",
            "--channel-settings",
            r#"{"twitter":{}}"#,
        ],
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unknown channel"), "err: {err}");
}

#[tokio::test]
async fn monitor_scoping_shorthand_and_get() {
    let app = common::spawn_app().await;
    cli(&app, &["workspaces", "create", "WS"]).await.unwrap();

    // --subreddit / --only-repo are shorthand for channel_settings keys.
    let out = cli(
        &app,
        &[
            "--json",
            "monitors",
            "create",
            "render farm",
            "--channel",
            "reddit",
            "--subreddit",
            "blender",
            "--subreddit",
            "vfx",
            "--only-repo",
            "my-org/*",
            "--ai-prompt",
            "Asking for render farm recommendations?",
        ],
    )
    .await
    .unwrap();
    let m: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        m["channel_settings"]["reddit"]["subreddits"],
        serde_json::json!(["blender", "vfx"])
    );
    assert_eq!(
        m["channel_settings"]["github"]["only_repos"],
        serde_json::json!(["my-org/*"])
    );
    let id = m["id"].as_str().unwrap().to_string();

    // `monitors get` shows the full detail: scoping JSON and the whole prompt.
    let out = cli(&app, &["monitors", "get", &id]).await.unwrap();
    assert!(out.contains("\"render farm\""), "out: {out}");
    assert!(out.contains("blender"), "out: {out}");
    assert!(
        out.contains("Asking for render farm recommendations?"),
        "out: {out}"
    );

    // Updating subreddits via shorthand replaces only reddit's scoping; the
    // github key survives because the CLI merges into the current settings.
    let out = cli(
        &app,
        &["--json", "monitors", "update", &id, "--subreddit", "Maya"],
    )
    .await
    .unwrap();
    let m: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        m["channel_settings"]["reddit"]["subreddits"],
        serde_json::json!(["Maya"])
    );
    assert_eq!(
        m["channel_settings"]["github"]["only_repos"],
        serde_json::json!(["my-org/*"])
    );
}

#[tokio::test]
async fn mentions_ai_views() {
    let app = common::spawn_app().await;
    cli(&app, &["workspaces", "create", "WS"]).await.unwrap();
    let out = cli(&app, &["--json", "monitors", "create", "brand"])
        .await
        .unwrap();
    let monitor: serde_json::Value = serde_json::from_str(&out).unwrap();
    let monitor_id = monitor["id"].as_str().unwrap();

    insert_mention(&app, monitor_id, "visible brand mention", "t3_vis").await;
    insert_mention_with_verdict(
        &app,
        monitor_id,
        "held brand mention",
        "t3_pend",
        Some("pending"),
    )
    .await;

    // Default view = feed-visible only (no verdict or accepted).
    let out = cli(&app, &["mentions", "list"]).await.unwrap();
    assert!(out.contains("visible brand mention"), "out: {out}");
    assert!(!out.contains("held brand mention"), "out: {out}");

    // --ai all shows the held one with its verdict marker.
    let out = cli(&app, &["mentions", "list", "--ai", "all"])
        .await
        .unwrap();
    assert!(out.contains("held brand mention"), "out: {out}");
    assert!(out.contains("[ai:pending]"), "out: {out}");

    // --ai pending shows exactly that verdict.
    let out = cli(&app, &["mentions", "list", "--ai", "pending"])
        .await
        .unwrap();
    assert!(out.contains("held brand mention"), "out: {out}");
    assert!(!out.contains("visible brand mention"), "out: {out}");

    // Invalid values are rejected at parse time.
    assert!(Cli::try_parse_from(["pulp", "mentions", "list", "--ai", "bogus"]).is_err());
}

/// `query` runs the real HackerNews collector against the mock Algolia server
/// — no Pulp server data involved — and returns the live results.
#[tokio::test]
#[serial]
async fn query_searches_a_channel_live() {
    let app = common::spawn_app().await;
    let mock_url = common::mock_hn::spawn().await;
    std::env::set_var("HACKERNEWS_BASE_URL", &mock_url);

    // Mock pages: 2 hits within 3 days (a comment and a "Show HN" story).
    let out = cli(
        &app,
        &[
            "query",
            "monitoring",
            "--channel",
            "hackernews",
            "--since",
            "3d",
        ],
    )
    .await
    .unwrap();
    std::env::remove_var("HACKERNEWS_BASE_URL");
    assert!(out.contains("hackernews: fetched 2"), "out: {out}");
    assert!(out.contains("Show HN"), "out: {out}");

    // Unknown channel fails fast, before any network call.
    let err = cli(&app, &["query", "x", "--channel", "twitter"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("available channels"), "err: {err}");
}

/// `logs` locates the server log via the app home (no server needed) so a
/// CLI-only agent can find and read it. Uses PULP_HOME, hence #[serial].
#[tokio::test]
#[serial]
async fn logs_command_locates_and_tails_the_server_log() {
    let app = common::spawn_app().await;
    let home = std::env::temp_dir().join(format!("pulp-logs-test-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("server.log"), "line one\nline two\nline three\n").unwrap();
    std::env::set_var("PULP_HOME", &home);

    // --path prints just the path (script-friendly).
    let out = cli(&app, &["logs", "--path"]).await.unwrap();
    assert!(out.trim().ends_with("server.log"), "out: {out}");

    // Default output: path header plus the last N lines.
    let out = cli(&app, &["logs", "--tail", "2"]).await.unwrap();
    assert!(out.contains("server.log"), "out: {out}");
    assert!(!out.contains("line one"), "out: {out}");
    assert!(
        out.contains("line two") && out.contains("line three"),
        "out: {out}"
    );

    // --json is machine-readable.
    let out = cli(&app, &["--json", "logs", "--tail", "1"]).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["exists"], true);
    assert_eq!(v["lines"].as_array().unwrap().len(), 1);
    assert_eq!(v["lines"][0], "line three");
    assert!(v["path"].as_str().unwrap().ends_with("server.log"));

    // A missing log file is reported, not an error.
    std::fs::remove_file(home.join("server.log")).unwrap();
    let out = cli(&app, &["logs"]).await.unwrap();
    assert!(out.contains("no log file yet"), "out: {out}");

    std::env::remove_var("PULP_HOME");
    let _ = std::fs::remove_dir_all(&home);
}
