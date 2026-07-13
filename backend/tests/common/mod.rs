pub mod mock_github;
pub mod mock_hn;
pub mod mock_push;
pub mod mock_reddit;
pub mod mock_sink;

use std::future::IntoFuture;
use std::sync::Arc;
use tokio::sync::broadcast;

use pulp::{
    api,
    config::Config,
    db::{
        pool::create_pool,
        repos::{
            channel::SqliteChannelRepo, collector_target::SqliteCollectorTargetRepo,
            mention::SqliteMentionRepo, monitor::SqliteMonitorRepo,
            notification::SqliteNotificationRepo, workspace::SqliteWorkspaceRepo,
        },
    },
    notifier::webpush::VapidKeys,
    state::AppState,
};

pub struct TestApp {
    pub base_url: String,
    pub client: reqwest::Client,
    pub state: std::sync::Arc<pulp::state::AppState>,
}

impl TestApp {
    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{}", self.base_url, path))
    }
    pub fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.post(format!("{}{}", self.base_url, path))
    }
    pub fn put(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.put(format!("{}{}", self.base_url, path))
    }
    pub fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.delete(format!("{}{}", self.base_url, path))
    }
}

pub async fn spawn_app() -> TestApp {
    spawn_app_with_ai(None).await
}

pub async fn spawn_app_with_ai(ai: Option<std::sync::Arc<dyn pulp::ai::AiJudge>>) -> TestApp {
    let db_name = uuid::Uuid::new_v4().to_string().replace('-', "");
    let database_url = format!("sqlite:file:{}?mode=memory&cache=shared", db_name);

    let pool = create_pool(&database_url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();

    let http = reqwest::Client::builder()
        .user_agent("Pulp-Test/0.1")
        .build()
        .unwrap();

    let (sse_tx, _) = broadcast::channel::<String>(16);

    // Sandboxed home so any config persistence (e.g. the settings API writing
    // config.json) stays inside a temp dir instead of the cwd.
    let mut config = Config::default();
    config.home = std::env::temp_dir().join(format!("pulp-test-home-{}", db_name));
    std::fs::create_dir_all(&config.home).ok();

    let state = Arc::new(AppState {
        config,
        http,
        sse_tx,
        pool: pool.clone(),
        workspaces: Arc::new(SqliteWorkspaceRepo::new(pool.clone())),
        monitors: Arc::new(SqliteMonitorRepo::new(pool.clone())),
        mentions: Arc::new(SqliteMentionRepo::new(pool.clone())),
        notifications: Arc::new(SqliteNotificationRepo::new(pool.clone())),
        channels: Arc::new(SqliteChannelRepo::new(pool.clone())),
        collector_targets: Arc::new(SqliteCollectorTargetRepo::new(pool.clone())),
        throttles: pulp::collectors::scheduler::default_throttles(),
        vapid: Arc::new(VapidKeys::generate()),
        ai: std::sync::RwLock::new(ai),
        ai_filter: std::sync::RwLock::new(pulp::config::AiFilterSettings::default()),
    });

    let app = api::router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app).into_future());

    let base_url = format!("http://{}", addr);
    let client = reqwest::Client::new();

    // No authentication â€” the API is open.
    TestApp {
        base_url,
        client,
        state,
    }
}
