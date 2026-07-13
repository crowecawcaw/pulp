use axum::{
    routing::{delete, get, post, put},
    Router,
};
use std::sync::Arc;
use utoipa::OpenApi;

use crate::state::AppState;

pub mod admin;
pub mod channels;
pub mod config;
pub mod logs;
pub mod mentions;
pub mod monitors;
pub mod notifications;
pub mod push;
pub mod sse;
pub mod targets;
pub mod workspaces;

/// Aggregated OpenAPI document. Rust is the single source of truth for the API
/// contract; the frontend regenerates its TypeScript types from this spec
/// (see `frontend/src/api/types.gen.ts` and the `gen:api` npm scripts).
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Pulp API",
        version = "0.1.0",
        description = "Pulp social-listening HTTP API. This spec is generated from the Rust backend and is the authoritative API contract."
    ),
    paths(
        workspaces::list,
        workspaces::create,
        workspaces::update,
        workspaces::delete,
        monitors::list,
        monitors::get,
        monitors::create,
        monitors::update,
        monitors::delete,
        mentions::list,
        mentions::pending_count,
        mentions::get,
        mentions::set_read,
        notifications::list,
        notifications::create,
        notifications::delete,
        notifications::send_test,
        channels::list,
        channels::get,
        channels::upsert,
        channels::cleanup,
        channels::backfill,
        targets::list_targets,
        push::vapid_public_key,
        admin::trigger_collect,
        admin::trigger_notify,
        admin::trigger_backfill,
        config::get_ai_config,
        config::update_ai_config,
        config::test_ai_config,
        logs::get_logs,
    ),
    components(schemas(
        crate::db::repos::traits::Workspace,
        crate::db::repos::traits::Monitor,
        crate::db::repos::traits::CreateMonitor,
        crate::db::repos::traits::UpdateMonitor,
        crate::db::repos::traits::Mention,
        crate::db::repos::traits::Notification,
        crate::db::repos::traits::CreateNotification,
        crate::db::repos::traits::ChannelConfig,
        crate::db::repos::traits::CollectorTarget,
        crate::db::repos::traits::BackfillJob,
        targets::TargetStatus,
        targets::TargetsResponse,
        targets::ThrottleState,
        crate::collectors::scheduler::TargetHealth,
        crate::collectors::scheduler::ChannelHealthSummary,
        push::VapidPublicKey,
        notifications::TestNotificationResult,
        workspaces::WorkspaceBody,
        mentions::MentionPage,
        mentions::SetReadRequest,
        crate::db::repos::traits::PendingCount,
        channels::ChannelBody,
        channels::CleanupBody,
        channels::MentionSample,
        channels::CleanupPreview,
        channels::CleanupResult,
        channels::CleanupResponse,
        channels::BackfillBody,
        channels::BackfillResult,
        admin::BackfillRequest,
        config::AiConfigView,
        config::AiConfigUpdate,
        config::AiTestResult,
        logs::LogResponse,
    )),
    tags(
        (name = "workspaces", description = "Workspace management"),
        (name = "monitors", description = "Monitor (watch) management"),
        (name = "mentions", description = "Ingested mentions feed"),
        (name = "notifications", description = "Per-workspace delivery endpoints (webpush/webhook)"),
        (name = "channels", description = "Channel configuration and maintenance"),
        (name = "push", description = "Web Push VAPID key (browser subscribe bootstrap)"),
        (name = "admin", description = "Operational / admin triggers"),
        (name = "config", description = "Runtime configuration (AI relevance filter)"),
        (name = "logs", description = "Per-service recent log output"),
    )
)]
pub struct ApiDoc;

// Deliberately NO CORS layer: the API is unauthenticated, so the browser's
// same-origin policy is the only thing stopping a malicious web page from
// reading mentions/credentials or rewriting config on a listener it can reach
// (loopback or tailnet). Nothing legitimate needs cross-origin access — the
// production UI is embedded and served same-origin, and the Vite dev server
// proxies `/api` (frontend/vite.config.ts). See docs/THREAT_MODEL.md before
// relaxing this; enforced by tests/test_cross_origin.rs.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // Workspaces
        .route("/api/workspaces", get(workspaces::list))
        .route("/api/workspaces", post(workspaces::create))
        .route("/api/workspaces/:id", put(workspaces::update))
        .route("/api/workspaces/:id", delete(workspaces::delete))
        // Monitors
        .route("/api/monitors", get(monitors::list))
        .route("/api/monitors", post(monitors::create))
        .route("/api/monitors/:id", get(monitors::get))
        .route("/api/monitors/:id", put(monitors::update))
        .route("/api/monitors/:id", delete(monitors::delete))
        // Mentions
        .route("/api/mentions", get(mentions::list))
        .route("/api/mentions/pending-count", get(mentions::pending_count))
        .route("/api/mentions/stream", get(sse::stream))
        .route("/api/mentions/:id", get(mentions::get))
        .route("/api/mentions/:id/read", put(mentions::set_read))
        // Notifications (per-workspace delivery endpoints)
        .route("/api/notifications", get(notifications::list))
        .route("/api/notifications", post(notifications::create))
        .route("/api/notifications/test", post(notifications::send_test))
        .route("/api/notifications/:id", delete(notifications::delete))
        // Channels
        .route("/api/channels", get(channels::list))
        .route(
            "/api/channels/:channel",
            get(channels::get).put(channels::upsert),
        )
        .route("/api/channels/:channel/cleanup", post(channels::cleanup))
        .route("/api/channels/:channel/backfill", post(channels::backfill))
        .route("/api/channels/:channel/targets", get(targets::list_targets))
        // Web Push: the browser fetches the VAPID public key before subscribing;
        // the resulting subscription is registered as a webpush notification.
        .route("/api/push/vapid-public-key", get(push::vapid_public_key))
        // Admin
        .route("/api/admin/collect/:channel", post(admin::trigger_collect))
        .route("/api/admin/notify", post(admin::trigger_notify))
        .route("/api/admin/backfill", post(admin::trigger_backfill))
        // Config (AI relevance filter settings)
        .route(
            "/api/config/ai",
            get(config::get_ai_config).put(config::update_ai_config),
        )
        .route("/api/config/ai/test", post(config::test_ai_config))
        // Per-service logs (channels today, ai_filter/llm later)
        .route("/api/logs/:service", get(logs::get_logs))
        // The OpenAPI spec is NOT served by the binary (Swagger UI is dropped
        // from the build to keep it network-free). Rust is still the source of
        // truth for the contract: generate the spec with `pulp --dump-openapi`
        // (see `ApiDoc` above) and package Swagger UI as a release artifact.
        // Everything else is the embedded web UI (SPA fallback included), so
        // one port serves the whole app — see `crate::web`.
        .fallback(crate::web::static_handler)
        .with_state(state)
}

#[cfg(test)]
mod openapi_tests {
    use super::*;

    /// The aggregated spec must build and expose the core resource paths. This
    /// guards against a handler being added without registering its
    /// `#[utoipa::path]` in `ApiDoc`.
    #[test]
    fn spec_builds_with_expected_paths() {
        let doc = ApiDoc::openapi();
        let paths = &doc.paths.paths;
        for expected in [
            "/api/workspaces",
            "/api/monitors",
            "/api/mentions",
            "/api/notifications",
            "/api/channels",
        ] {
            assert!(paths.contains_key(expected), "missing path: {expected}");
        }

        // The component schemas the frontend types are generated from.
        let schemas = doc
            .components
            .as_ref()
            .expect("components present")
            .schemas
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for expected in [
            "Workspace",
            "Monitor",
            "Mention",
            "Notification",
            "ChannelConfig",
        ] {
            assert!(
                schemas.iter().any(|s| s == expected),
                "missing schema: {expected}"
            );
        }

        // Must serialize cleanly to JSON (what we serve + dump).
        assert!(doc.to_pretty_json().is_ok());
    }
}
