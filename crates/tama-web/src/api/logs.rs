//! Backend log file reading endpoint: GET /tama/v1/logs/:backend
//!
//! Note: SSE streaming (GET /tama/v1/logs/:backend/events) is handled by the
//! tama-core proxy, not this web UI server. The web UI proxies those requests
//! to the proxy via the catch-all handler.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::server::AppState;

/// Maximum number of lines to return (clamp for the `lines` query parameter).
pub const MAX_LINES: usize = 10_000;

/// Query parameters for GET /tama/v1/logs/:backend
#[derive(Deserialize)]
pub struct BackendLogsQuery {
    /// Number of lines to return (default: 200)
    #[serde(default = "default_lines")]
    pub lines: usize,
}

fn default_lines() -> usize {
    200
}

/// Validate a backend name for use in log file paths.
pub fn is_valid_backend_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// GET /tama/v1/logs/:backend — return the last N lines of a backend's log file.
pub async fn get_backend_logs(
    State(state): State<Arc<AppState>>,
    Path(backend): Path<String>,
    Query(query): Query<BackendLogsQuery>,
) -> impl IntoResponse {
    let dir = match &state.logs_dir {
        Some(d) => d.clone(),
        None => {
            let config_dir = match &state.config_path {
                Some(p) => p.parent().map(|d| d.to_path_buf()),
                None => None,
            };
            match config_dir {
                Some(dir) => dir.join("logs"),
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(serde_json::json!({"error": "logs_dir not configured"})),
                    )
                        .into_response()
                }
            }
        }
    };

    if !is_valid_backend_name(&backend) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid backend name"})),
        )
            .into_response();
    }

    let path = dir.join(format!("{}.log", backend));

    if !path.exists() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("No logs found for '{}'", backend)})),
        )
            .into_response();
    }

    let n = query.lines.min(MAX_LINES);
    let path_clone = path.clone();
    let lines =
        tokio::task::spawn_blocking(move || tama_core::logging::tail_lines(&path_clone, n)).await;

    match lines {
        Ok(Ok(result)) => Json(serde_json::json!({ "lines": result })).into_response(),
        Ok(Err(e)) => {
            tracing::warn!("Failed to read backend log {}: {}", path.display(), e);
            Json(serde_json::json!({ "lines": Vec::<String>::new() })).into_response()
        }
        Err(join_err) => {
            tracing::warn!(
                "Failed to read backend log {} (spawn_blocking): {}",
                path.display(),
                join_err
            );
            Json(serde_json::json!({ "lines": Vec::<String>::new() })).into_response()
        }
    }
}
