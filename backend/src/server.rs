//! Server startup: wiring config, database, collectors, notifier, and the
//! Axum router. Invoked by `pulp serve`.

use std::sync::Arc;

use crate::ai;
use crate::api;
use crate::collectors;
use crate::config::Config;
use crate::db;
use crate::notifier;
use crate::state::AppState;

/// Load config, open the database, spawn the background collectors/notifier,
/// and serve the HTTP API until `shutdown` resolves (SIGINT/SIGTERM, wired by
/// `cli::serve`) or the process is killed.
pub async fn run(
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    // Resolve ~/.pulp (or PULP_HOME), create its directory tree,
    // load/create <home>/config.json, then apply env-var overrides.
    let config = Config::load()?;

    // Ensure the database's parent directory exists before sqlx connects
    // (covers DATABASE_URL / database_path values outside the app home).
    if let Some(path) = config.database_url.strip_prefix("sqlite:") {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
    }

    let pool = db::pool::create_pool(&config.database_url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // One-time repair of legacy monitor OR-phrases into the new bare-term model
    // (a since-squashed migration backfilled `terms = [phrase]`; this splits any
    // element that was a malformed `"a" OR "b"` string the old matcher never
    // matched). Idempotent — a no-op once every monitor is already in the new
    // shape (true for every fresh database, since `terms` starts at `[]`).
    match db::repos::monitor::repair_legacy_or_phrases(&pool).await {
        Ok(0) => {}
        Ok(n) => tracing::info!("repaired {} legacy OR-phrase monitor(s) into term lists", n),
        Err(e) => tracing::error!("legacy OR-phrase repair failed: {:?}", e),
    }

    // WAL mode for better concurrency. (`foreign_keys` is set per-connection
    // by `db::pool::create_pool` via `SqliteConnectOptions::foreign_keys`.)
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await?;

    // The timeouts matter: reqwest's default has NONE, and a throttled client
    // can be tarpitted (connection accepted, response never sent) — observed
    // live with Reddit's CDN — hanging that channel's collector task forever.
    let http = reqwest::Client::builder()
        .user_agent("Pulp/0.1")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let (sse_tx, _) = tokio::sync::broadcast::channel::<String>(256);

    // The optional AI relevance judge is a bring-your-own OpenAI-compatible
    // endpoint, derived on demand from `ai_filter` settings (held behind a lock
    // in AppState so the settings API/CLI can change it without a restart).
    // `None` => AI criteria leaves fail closed.
    let ai_mechanism = if ai::judge_from_config(&config.ai_filter).is_some() {
        format!(
            "enabled (model '{}' at {})",
            config.ai_filter.model, config.ai_filter.base_url
        )
    } else if config.ai_filter.enabled {
        "disabled (enabled but base_url/model incomplete)".to_string()
    } else {
        "disabled".to_string()
    };

    tracing::info!(
        "Pulp home: {} | database: {} | config: {} | ai_filter: {}",
        config.home.display(),
        config.database_url,
        config.config_source,
        ai_mechanism
    );

    // VAPID identity for Web Push: persisted in the app home, generated on
    // first run. Rotating it would invalidate every existing subscription, so
    // a malformed key file is fatal rather than silently regenerated.
    let vapid = Arc::new(notifier::webpush::VapidKeys::load_or_create(
        &config.home,
        &config.vapid_subject,
    )?);

    let state = Arc::new(AppState {
        config: config.clone(),
        http: http.clone(),
        sse_tx: sse_tx.clone(),
        pool: pool.clone(),
        workspaces: Arc::new(db::repos::workspace::SqliteWorkspaceRepo::new(pool.clone())),
        monitors: Arc::new(db::repos::monitor::SqliteMonitorRepo::new(pool.clone())),
        mentions: Arc::new(db::repos::mention::SqliteMentionRepo::new(pool.clone())),
        notifications: Arc::new(db::repos::notification::SqliteNotificationRepo::new(
            pool.clone(),
        )),
        channels: Arc::new(db::repos::channel::SqliteChannelRepo::new(pool.clone())),
        collector_targets: Arc::new(db::repos::collector_target::SqliteCollectorTargetRepo::new(
            pool.clone(),
        )),
        throttles: collectors::scheduler::default_throttles(),
        vapid,
        ai: std::sync::RwLock::new(ai::judge_from_config(&config.ai_filter)),
        ai_filter: std::sync::RwLock::new(config.ai_filter.clone()),
    });

    collectors::spawn_all(state.clone());
    notifier::spawn(state.clone());
    crate::ai_filter::spawn(state.clone());

    let app = api::router(state);

    spawn_https(&config, app.clone());

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!("Listening on http://{}", config.bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    tracing::info!("server stopped");
    Ok(())
}

/// How often the server re-checks for certificates while HTTPS is wanted but
/// not yet resolvable — e.g. the user enables Tailscale HTTPS certificates
/// after startup. HTTPS then comes up by itself; no restart or command.
const HTTPS_RECHECK_SECS: u64 = 5 * 60;

/// Start the HTTPS listener when certificates resolve (explicit paths or
/// Tailscale-provisioned — see [`crate::tls`]); until they do, keep
/// re-checking in the background. Never fatal: the HTTP listener always runs.
fn spawn_https(config: &Config, app: axum::Router) {
    if config.https.mode == "off" {
        return;
    }
    let config = config.clone();
    tokio::spawn(async move {
        let mut hinted = false;
        loop {
            // tls::resolve shells out to the tailscale CLI — keep it off the
            // async runtime threads.
            let cfg = config.clone();
            let resolved = tokio::task::spawn_blocking(move || crate::tls::resolve(&cfg))
                .await
                .ok()
                .flatten();
            if let Some(resolved) = resolved {
                serve_https(&config, app, resolved).await;
                return;
            }
            if !hinted {
                hinted = true;
                let port = config.bind.rsplit(':').next().unwrap_or("3000");
                let msg = format!(
                    "HTTPS not enabled yet (no certificates resolved); re-checking every \
                     {} min. Easiest setup: install Tailscale and enable HTTPS \
                     certificates in its admin console — pulp provisions certs \
                     itself once that's on. Alternatives: `tailscale serve --bg {}` \
                     to let tailscaled terminate TLS, or set \
                     server.https.cert_path/key_path in config.json.",
                    HTTPS_RECHECK_SECS / 60,
                    port
                );
                if config.https.mode == "on" {
                    tracing::error!("{}", msg);
                } else {
                    tracing::info!("{}", msg);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(HTTPS_RECHECK_SECS)).await;
        }
    });
}

/// Run the HTTPS listener with resolved certs (plus the daily Tailscale
/// refresh loop). Returns only on listener setup failure.
async fn serve_https(config: &Config, app: axum::Router, resolved: crate::tls::ResolvedTls) {
    // axum-server is built without a crypto provider (keeps aws-lc-rs and its
    // cmake/nasm build deps out); install ring as the process default.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let host = config
        .bind
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or("0.0.0.0");
    let display_host = resolved.host.clone().unwrap_or_else(|| host.to_string());
    // A loopback `server.host` can't be reached via the cert's tailnet name
    // (it resolves to the Tailscale IP), so bind there instead — tailnet-
    // reachable without exposing the unauthenticated API LAN-wide. An
    // explicit non-loopback host is respected as configured.
    let loopback = host == "127.0.0.1" || host == "localhost" || host == "::1";
    let https_host = match (&resolved.bind_ip, loopback) {
        (Some(ip), true) => ip.to_string(),
        _ => host.to_string(),
    };
    let addr = format!("{}:{}", https_host, config.https.port);
    let https_port = config.https.port;
    let home = config.home.clone();

    let rustls_config =
        match axum_server::tls_rustls::RustlsConfig::from_pem_file(&resolved.cert, &resolved.key)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(
                    "failed to load TLS cert {} / key {}: {}; HTTPS disabled",
                    resolved.cert.display(),
                    resolved.key.display(),
                    e
                );
                return;
            }
        };

    // Tailscale certs are short-lived (Let's Encrypt): re-run the
    // provisioning daily and hot-reload the listener's config.
    if resolved.tailscale {
        let reload = rustls_config.clone();
        let (cert, key) = (resolved.cert.clone(), resolved.key.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(24 * 60 * 60)).await;
                let home = home.clone();
                let _ =
                    tokio::task::spawn_blocking(move || crate::tls::resolve_tailscale(&home)).await;
                if let Err(e) = reload.reload_from_pem_file(&cert, &key).await {
                    tracing::warn!("TLS cert reload failed: {}", e);
                }
            }
        });
    }

    let socket_addr: std::net::SocketAddr = match addr.parse() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("invalid HTTPS bind address {}: {}; HTTPS disabled", addr, e);
            return;
        }
    };
    tracing::info!("Listening on https://{}:{}", display_host, https_port);
    if let Err(e) = axum_server::bind_rustls(socket_addr, rustls_config)
        .serve(app.into_make_service())
        .await
    {
        tracing::error!("HTTPS listener failed: {}", e);
    }
}
