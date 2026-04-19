use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use crate::server::AppState;

pub mod backends;
pub mod backup;
pub mod benchmarks;
pub mod downloads;
pub mod middleware;
pub mod models;
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
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> impl IntoResponse {
    let dir = match &state.logs_dir {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "logs_dir not configured"})),
            )
                .into_response()
        }
    };
    let log_path = dir.join("koji.log");
    // Use spawn_blocking for synchronous file I/O to avoid blocking the Tokio runtime.
    let log_path_clone = log_path.clone();
    let n = query.lines;
    let lines = tokio::task::spawn_blocking(move || {
        koji_core::logging::tail_lines(&log_path_clone, n).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    Json(serde_json::json!({ "lines": lines })).into_response()
}

pub async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let path = match &state.config_path {
        Some(p) => p.clone(),
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
/// No-op if proxy_config is None (standalone web server without proxy).
async fn sync_proxy_config(state: &AppState, new_config: koji_core::config::Config) {
    if let Some(ref proxy_config) = state.proxy_config {
        let mut config = proxy_config.write().await;
        *config = new_config;
    }
}

/// Trigger the proxy to reload its model registry from the database.
async fn trigger_proxy_reload(state: &AppState) -> Result<(), (StatusCode, serde_json::Value)> {
    let url = format!("{}/koji/v1/system/reload-configs", state.proxy_base_url);
    let resp = state.client.post(&url).send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            serde_json::json!({"error": format!("Failed to reach proxy: {}", e)}),
        )
    })?;

    if !resp.status().is_success() {
        return Err((
            resp.status(),
            serde_json::json!({"error": "Proxy failed to reload configurations"}),
        ));
    }
    Ok(())
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
    State(state): State<Arc<AppState>>,
    Json(body): Json<ConfigBody>,
) -> impl IntoResponse {
    let path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response()
        }
    };
    // Validate TOML by parsing. Note: koji_core::config::Config has required fields
    // (e.g. `general`), so a partial TOML that omits top-level tables will fail here.
    // This is intentional — only fully valid config files are accepted.
    if let Err(e) = toml::from_str::<koji_core::config::Config>(&body.content) {
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
                toml::from_str::<koji_core::config::Config>(&content_for_sync)
            {
                // Restore loaded_from from the existing proxy config (it is skipped by serde).
                if let Some(ref proxy_config) = state.proxy_config {
                    new_config.loaded_from = proxy_config.read().await.loaded_from.clone();
                }
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
pub async fn get_structured_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
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

    // Load config from disk using koji_core (SSR-only path)
    let cfg = match tokio::task::spawn_blocking(move || {
        koji_core::config::Config::load_from(&config_dir)
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
    State(state): State<Arc<AppState>>,
    Json(body): Json<StructuredConfigBody>,
) -> impl IntoResponse {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
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

    // Convert mirror types back to koji_core::Config
    let mut new_config: koji_core::config::Config = body.into();

    // Restore loaded_from from existing proxy config (it has #[serde(skip)])
    if let Some(ref proxy_config) = state.proxy_config {
        new_config.loaded_from = proxy_config.read().await.loaded_from.clone();
    }

    // Persist to disk using koji_core's save_to (consistent with other endpoints)
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

/// Load config from the config_path stored in AppState.
/// Returns (config, config_dir) on success.
fn load_config_from_state(
    state: &AppState,
) -> Result<(koji_core::config::Config, std::path::PathBuf), (StatusCode, serde_json::Value)> {
    let config_path = state.config_path.as_ref().ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            serde_json::json!({"error": "config_path not configured"}),
        )
    })?;
    let config_dir = config_path
        .parent()
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": "Cannot determine config directory"}),
            )
        })?
        .to_path_buf();
    let cfg = koji_core::config::Config::load_from(&config_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}),
        )
    })?;
    Ok((cfg, config_dir))
}
