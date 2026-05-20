use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use tama_core::proxy::ProxyState;

pub mod backends;
pub mod backup;
pub mod benchmarks;
pub mod downloads;
pub mod hf;
pub mod logs;
pub mod middleware;
pub mod models;
pub mod openapi;
pub mod self_update;
pub mod updates;

// Re-export for backward compatibility
pub use models::*;

/// Query parameters for GET /api/logs
#[derive(serde::Deserialize)]
pub struct LogsQuery {
    /// Number of lines to return (default: 200)
    #[serde(default = "default_lines")]
    pub lines: usize,
}
fn default_lines() -> usize {
    200
}

pub async fn get_logs(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> impl IntoResponse {
    let dir = match state.config.read().await.logs_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    let log_path = dir.join("tama.log");
    // Use spawn_blocking for synchronous file I/O to avoid blocking the Tokio runtime.
    let log_path_clone = log_path.clone();
    let n = query.lines;
    let lines = tokio::task::spawn_blocking(move || {
        tama_core::logging::tail_lines(&log_path_clone, n).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    Json(serde_json::json!({ "lines": lines })).into_response()
}

pub async fn get_config(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let path = match state.config.read().await.loaded_from.clone() {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response()
        }
    };
    // Use spawn_blocking for synchronous file I/O.
    match tokio::task::spawn_blocking(move || std::fs::read_to_string(&path)).await {
        Ok(Ok(content)) => Json(serde_json::json!({ "content": content })).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct ConfigBody {
    pub content: String,
}

/// Update the proxy's live in-memory config after a successful disk save.
async fn sync_proxy_config(state: &ProxyState, new_config: tama_core::config::Config) {
    let mut config = state.config.write().await;
    *config = new_config;
}

/// Trigger the proxy to reload its model registry from the database.
async fn trigger_proxy_reload(state: &ProxyState) -> Result<(), (StatusCode, serde_json::Value)> {
    state.reload_model_configs().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": format!("Failed to reload model configs: {}", e)}),
        )
    })
}

/// Body for structured config save.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StructuredConfigBody {
    pub general: crate::types::config::General,
    #[serde(default)]
    pub backends: std::collections::BTreeMap<String, crate::types::config::BackendConfig>,
    #[serde(default)]
    pub models: std::collections::BTreeMap<String, crate::types::config::ModelConfig>,
    #[serde(default)]
    pub supervisor: crate::types::config::Supervisor,
    #[serde(default)]
    pub sampling_templates:
        std::collections::BTreeMap<String, crate::types::config::SamplingParams>,
    #[serde(default)]
    pub proxy: crate::types::config::ProxyConfig,
}

pub async fn save_config(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<ConfigBody>,
) -> impl IntoResponse {
    let path = match state.config.read().await.loaded_from.clone() {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response()
        }
    };
    // Validate TOML by parsing. Note: tama_core::config::Config has required fields
    // (e.g. `general`), so a partial TOML that omits top-level tables will fail here.
    // This is intentional — only fully valid config files are accepted.
    if let Err(e) = toml::from_str::<tama_core::config::Config>(&body.content) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": format!("Invalid TOML: {e}")})),
        )
            .into_response();
    }
    // Keep a copy of the validated content for syncing after the write.
    let content_for_sync = body.content.clone();
    // Use spawn_blocking for synchronous file I/O.
    match tokio::task::spawn_blocking(move || std::fs::write(&path, &body.content)).await {
        Ok(Ok(_)) => {
            // Parse the validated TOML into a Config and sync the proxy's live config.
            if let Ok(mut new_config) =
                toml::from_str::<tama_core::config::Config>(&content_for_sync)
            {
                // Restore loaded_from from the existing config (it is skipped by serde).
                new_config.loaded_from = state.config.read().await.loaded_from.clone();
                sync_proxy_config(&state, new_config).await;
            }
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Structured Config API (JSON-based for WASM) ─────────────────────────────────

/// GET /api/config/structured — returns full Config as JSON.
pub async fn get_structured_config(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let config_path = match state.config.read().await.loaded_from.clone() {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response();
        }
    };

    let config_dir = match config_path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Cannot determine config directory"})),
            )
                .into_response();
        }
    };

    // Load config from disk using tama_core (SSR-only path)
    let cfg = match tokio::task::spawn_blocking(move || {
        tama_core::config::Config::load_from(&config_dir)
    })
    .await
    {
        Ok(Ok(cfg)) => cfg,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Convert to mirror types for JSON serialization
    let structured: crate::types::config::Config = cfg.into();

    Json(structured).into_response()
}

/// POST /api/config/structured — accept JSON Config, persist as TOML.
pub async fn save_structured_config(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<StructuredConfigBody>,
) -> impl IntoResponse {
    let config_path = match state.config.read().await.loaded_from.clone() {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response();
        }
    };

    let config_dir = match config_path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Cannot determine config directory"})),
            )
                .into_response();
        }
    };

    // Convert mirror types back to tama_core::Config
    let mut new_config: tama_core::config::Config = body.into();

    // Restore loaded_from from existing config (it has #[serde(skip)])
    new_config.loaded_from = state.config.read().await.loaded_from.clone();

    // Persist to disk using tama_core's save_to (consistent with other endpoints)
    let config_dir_clone = config_dir.clone();
    let new_config_clone = new_config.clone();
    match tokio::task::spawn_blocking(move || new_config_clone.save_to(&config_dir_clone)).await {
        Ok(Ok(_)) => {
            // Sync proxy config for hot-reload
            sync_proxy_config(&state, new_config).await;
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Shared helpers (used by both model and non-model endpoints) ──────────────

/// Load config from the config directory derived from ProxyState.
/// Returns (config, config_dir) on success.
async fn load_config_from_state(
    state: &ProxyState,
) -> Result<(tama_core::config::Config, std::path::PathBuf), (StatusCode, serde_json::Value)> {
    // Prefer db_dir (set at startup to Config::config_dir()) to ensure we
    // always open the correct database. Fall back to loaded_from when db_dir
    // is None (e.g. in tests that create ProxyState without a db_dir).
    let config_dir = state
        .db_dir
        .clone()
        .or_else(|| {
            state.config.try_read().ok()?.loaded_from.clone()
        })
        .and_then(|loaded| {
            if loaded.is_dir() {
                Some(loaded)
            } else {
                loaded.parent().map(|p| p.to_path_buf())
            }
        })
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                serde_json::json!({"error": "config_path not configured"}),
            )
        })?;
    let config_dir_clone = config_dir.clone();
    let cfg = tokio::task::spawn_blocking(move || {
        tama_core::config::Config::load_from(&config_dir_clone)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}),
        )
    })?;
    Ok((cfg, config_dir))
}
