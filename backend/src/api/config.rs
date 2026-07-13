//! Settings API for the optional AI relevance filter (bring-your-own
//! OpenAI-compatible LLM endpoint).
//!
//! `GET /api/config/ai` returns the current settings (never the API key, only
//! whether one is set). `PUT` validates + persists them to config.json and
//! hot-swaps the live judge without a restart. `POST /api/config/ai/test` runs
//! one sample judgment so the UI/CLI can verify connectivity.

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::config::{AiFilterSection, AiFilterSettings};
use crate::error::AppError;
use crate::state::AppState;

/// AI-filter settings as exposed to clients. The API key is never returned;
/// `api_key_set` reports only whether one is configured.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AiConfigView {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub api_key_set: bool,
}

impl AiConfigView {
    fn from_settings(s: &AiFilterSettings) -> Self {
        Self {
            enabled: s.enabled,
            base_url: s.base_url.clone(),
            model: s.model.clone(),
            api_key_set: s.api_key.as_deref().is_some_and(|k| !k.trim().is_empty()),
        }
    }
}

/// Partial update. Every field is optional; omitted (or null) fields are left
/// unchanged. For `api_key`, an empty string clears the stored key.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AiConfigUpdate {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Result of a `test` call: a sample verdict on success, or an error message.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AiTestResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/config/ai",
    tag = "config",
    responses((status = 200, body = AiConfigView))
)]
pub async fn get_ai_config(State(state): State<Arc<AppState>>) -> Json<AiConfigView> {
    Json(AiConfigView::from_settings(&state.ai_filter()))
}

#[utoipa::path(
    put,
    path = "/api/config/ai",
    tag = "config",
    request_body = AiConfigUpdate,
    responses(
        (status = 200, body = AiConfigView),
        (status = 400, description = "Invalid settings (e.g. enabled without base_url/model)")
    )
)]
pub async fn update_ai_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AiConfigUpdate>,
) -> Result<Json<AiConfigView>, AppError> {
    let mut cfg = state.ai_filter();
    if let Some(enabled) = body.enabled {
        cfg.enabled = enabled;
    }
    if let Some(base_url) = body.base_url {
        cfg.base_url = base_url.trim().to_string();
    }
    if let Some(model) = body.model {
        cfg.model = model.trim().to_string();
    }
    if let Some(api_key) = body.api_key {
        cfg.api_key = Some(api_key).filter(|k| !k.trim().is_empty());
    }

    if cfg.enabled && cfg.base_url.is_empty() {
        return Err(AppError::BadRequest(
            "base_url is required to enable AI filtering".to_string(),
        ));
    }
    if cfg.enabled && cfg.model.is_empty() {
        return Err(AppError::BadRequest(
            "model is required to enable AI filtering".to_string(),
        ));
    }

    // Persist to config.json so the setting survives a restart, then hot-swap
    // the live judge to match.
    let section = AiFilterSection {
        enabled: cfg.enabled,
        base_url: cfg.base_url.clone(),
        model: cfg.model.clone(),
        api_key: cfg.api_key.clone(),
    };
    state
        .config
        .save_ai_filter(&section)
        .map_err(AppError::Internal)?;
    state.apply_ai_filter(cfg.clone());

    Ok(Json(AiConfigView::from_settings(&cfg)))
}

#[utoipa::path(
    post,
    path = "/api/config/ai/test",
    tag = "config",
    responses((status = 200, body = AiTestResult))
)]
pub async fn test_ai_config(State(state): State<Arc<AppState>>) -> Json<AiTestResult> {
    let Some(judge) = state.ai_judge() else {
        return Json(AiTestResult {
            ok: false,
            verdict: None,
            reason: None,
            error: Some("AI filtering is disabled or not fully configured".to_string()),
        });
    };

    // `judge()` blocks on a network round-trip; keep it off the async workers.
    let verdict = tokio::task::spawn_blocking(move || {
        judge.judge(
            "A self-hostable product-analytics and time-series database.",
            "Has anyone found a good way to self-host a time-series database for product analytics?",
        )
    })
    .await
    .ok()
    .flatten();

    match verdict {
        Some(v) => Json(AiTestResult {
            ok: true,
            verdict: Some(if v.score >= 0.5 { "include" } else { "exclude" }.to_string()),
            reason: v.reason,
            error: None,
        }),
        None => Json(AiTestResult {
            ok: false,
            verdict: None,
            reason: None,
            error: Some(
                "the endpoint did not return a valid verdict — check base_url, model, \
                 and api_key (details in server.log)"
                    .to_string(),
            ),
        }),
    }
}
