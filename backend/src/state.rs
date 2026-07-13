use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

use crate::ai::AiJudge;
use crate::config::{AiFilterSettings, Config};
use crate::db::repos::traits::{
    ChannelRepo, CollectorTargetRepo, MentionRepo, MonitorRepo, NotificationRepo, WorkspaceRepo,
};
use crate::notifier::webpush::VapidKeys;

pub struct AppState {
    pub config: Config,
    pub http: reqwest::Client,
    pub sse_tx: broadcast::Sender<String>,
    pub pool: sqlx::SqlitePool,
    pub workspaces: Arc<dyn WorkspaceRepo>,
    pub monitors: Arc<dyn MonitorRepo>,
    pub mentions: Arc<dyn MentionRepo>,
    pub notifications: Arc<dyn NotificationRepo>,
    pub channels: Arc<dyn ChannelRepo>,
    /// Durable collection targets + backfill jobs for the targeted (auto-
    /// recovering) collector runner.
    pub collector_targets: Arc<dyn CollectorTargetRepo>,
    /// Per-channel adaptive throttles, keyed by lane (e.g. `"reddit"`). Shared
    /// across passes so the AIMD rate state persists between cycles.
    pub throttles: crate::ratelimit::KeyedThrottle<String>,
    /// The server's VAPID identity for Web Push. Served to the browser at
    /// subscribe time and used to authorize every push.
    pub vapid: Arc<VapidKeys>,
    /// Active AI relevance judge (the optional bring-your-own LLM endpoint).
    /// `None` when AI filtering is disabled/unconfigured; AI criteria leaves
    /// fail closed when absent. Behind a lock so the settings API/CLI can
    /// hot-swap it without a restart, and so tests can inject a stub judge.
    pub ai: RwLock<Option<Arc<dyn AiJudge>>>,
    /// The settings the active judge was built from — the source for the
    /// settings API's view and the value persisted to config.json.
    pub ai_filter: RwLock<AiFilterSettings>,
}

impl AppState {
    /// Current relevance judge, cloned out of the lock for use off-thread.
    /// `None` means AI filtering is disabled/unconfigured (callers fail closed).
    pub fn ai_judge(&self) -> Option<Arc<dyn AiJudge>> {
        self.ai.read().expect("ai judge lock poisoned").clone()
    }

    /// Snapshot the current AI-filter settings (for the settings API view).
    pub fn ai_filter(&self) -> AiFilterSettings {
        self.ai_filter
            .read()
            .expect("ai_filter lock poisoned")
            .clone()
    }

    /// Apply new AI-filter settings: rebuild the active judge to match and store
    /// the settings. Used by the settings API/CLI after a config change.
    pub fn apply_ai_filter(&self, cfg: AiFilterSettings) {
        let judge = crate::ai::judge_from_config(&cfg);
        *self.ai.write().expect("ai judge lock poisoned") = judge;
        *self.ai_filter.write().expect("ai_filter lock poisoned") = cfg;
    }
}
