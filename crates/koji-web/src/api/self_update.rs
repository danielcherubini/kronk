//! Self-update API endpoints for the Koji web UI.
//!
//! Provides three endpoints:
//! - `GET /koji/v1/self-update/check` — check if a new version is available
//! - `POST /koji/v1/self-update/update` — trigger the update (CSRF-protected)
//! - `GET /koji/v1/self-update/events` — SSE stream of update progress

use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::server::AppState;

/// Response for `GET /koji/v1/self-update/check`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResponse {
    pub update_available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: String,
    pub published_at: String,
}

/// Response for `POST /koji/v1/self-update/update`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTriggerResponse {
    pub ok: bool,
    pub message: String,
}

/// Check whether a newer version of Koji is available on GitHub Releases.
///
/// Uses `state.binary_version` (the actual running binary version passed from
/// the CLI) rather than `env!("CARGO_PKG_VERSION")` to avoid version mismatch
/// between koji-web and koji-cli crate versions.
pub async fn check_update(
    State(state): State<Arc<AppState>>,
) -> Result<Json<UpdateCheckResponse>, (StatusCode, Json<serde_json::Value>)> {
    match koji_core::self_update::check_for_update(&state.binary_version).await {
        Ok(info) => Ok(Json(UpdateCheckResponse {
            update_available: info.update_available,
            current_version: info.current_version,
            latest_version: info.latest_version,
            release_notes: info.release_notes,
            published_at: info.published_at,
        })),
        Err(e) => Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("Failed to check for updates: {e}") })),
        )),
    }
}

/// Trigger a self-update in the background.
///
/// This endpoint is placed inside the `backend_routes` sub-router which has
/// `enforce_same_origin` middleware for CSRF protection.
///
/// The update runs asynchronously. Progress is streamed via
/// `GET /koji/v1/self-update/events` (SSE).
pub async fn trigger_update(
    State(state): State<Arc<AppState>>,
) -> Result<Json<UpdateTriggerResponse>, (StatusCode, Json<serde_json::Value>)> {
    // Check if an update is already in progress
    {
        let guard = state.update_tx.lock().await;
        if let Some(ref tx) = *guard {
            if tx.receiver_count() > 0 || !tx.is_empty() {
                return Err((
                    StatusCode::CONFLICT,
                    Json(json!({ "error": "An update is already in progress" })),
                ));
            }
        }
    }

    // Create a broadcast channel for progress messages
    let (tx, _) = broadcast::channel::<String>(64);

    // Store the sender in AppState so the SSE endpoint can subscribe
    {
        let mut guard = state.update_tx.lock().await;
        *guard = Some(tx.clone());
    }

    let binary_version = state.binary_version.clone();

    // Spawn the update task
    tokio::spawn(async move {
        let tx_clone = tx.clone();
        let state_clone = state.clone();
        let progress_callback = move |msg: String| {
            if let Err(e) = tx_clone.send(msg) {
                tracing::warn!("Failed to send update progress: {}", e);
            }
        };

        match koji_core::self_update::perform_update(&binary_version, progress_callback).await {
            Ok(result) => {
                let status_msg = json!({
                    "type": "status",
                    "status": "succeeded",
                    "old_version": result.old_version,
                    "new_version": result.new_version,
                })
                .to_string();
                if let Err(e) = tx.send(status_msg) {
                    tracing::warn!("Failed to send update success status: {}", e);
                }

                let restart_msg = json!({"type": "restarting"}).to_string();
                if let Err(e) = tx.send(restart_msg) {
                    tracing::warn!("Failed to send restart status: {}", e);
                }

                // Brief delay before restarting to allow clients to receive the events
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                // Restart the process — this call does not return on success
                if let Err(e) = koji_core::self_update::restart_process() {
                    tracing::error!("Failed to restart after update: {e}");
                    let error_msg = json!({
                        "type": "status",
                        "status": "failed",
                        "error": format!("Update succeeded but restart failed: {e}")
                    })
                    .to_string();
                    if let Err(err) = tx.send(error_msg) {
                        tracing::warn!("Failed to send restart failure status: {}", err);
                    }

                    // Cleanup: set update_tx back to None so future requests can start a new update
                    let mut guard = state_clone.update_tx.lock().await;
                    *guard = None;
                }
            }
            Err(e) => {
                tracing::error!("Self-update failed: {e:#}");
                let error_msg = json!({
                    "type": "status",
                    "status": "failed",
                    "error": format!("{e:#}")
                })
                .to_string();
                if let Err(err) = tx.send(error_msg) {
                    tracing::warn!("Failed to send update failure status: {}", err);
                }

                // Cleanup: set update_tx back to None so future requests can start a new update
                let mut guard = state_clone.update_tx.lock().await;
                *guard = None;
            }
        }
    });

    Ok(Json(UpdateTriggerResponse {
        ok: true,
        message: "Update started".to_string(),
    }))
}

/// SSE stream of update progress events.
///
/// Subscribes to the broadcast channel populated by `trigger_update`.
/// If no update is in progress, returns an immediate "idle" event and closes.
///
/// Event format matches the existing `job_events_sse` pattern:
/// - `event: log` with JSON `{ "line": "..." }` for progress messages
/// - `event: status` with JSON `{ "status": "succeeded"|"failed", ... }` for completion
/// - `event: restarting` with JSON `{}` before process restart
pub async fn update_events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, axum::Error>>> {
    let rx = {
        let guard = state.update_tx.lock().await;
        guard.as_ref().map(|tx| tx.subscribe())
    };

    let stream = async_stream::stream! {
        let Some(mut rx) = rx else {
            yield Ok(
                Event::default()
                    .event("status")
                    .json_data(json!({ "status": "idle" }))
                    .unwrap_or_default()
            );
            return;
        };

        loop {
            match rx.recv().await {
                Ok(message) => {
                    // Try to parse as JSON to detect structured messages
                    // (status/restarting events are sent as JSON strings)
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&message) {
                        if let Some(msg_type) = parsed.get("type").and_then(|t| t.as_str()) {
                            match msg_type {
                                "status" => {
                                    yield Ok(
                                        Event::default()
                                            .event("status")
                                            .json_data(parsed.clone())
                                            .unwrap_or_default()
                                    );
                                    // Terminal event — close the stream
                                    if parsed.get("status").and_then(|s| s.as_str()) == Some("failed") {
                                        break;
                                    }
                                }
                                "restarting" => {
                                    yield Ok(
                                        Event::default()
                                            .event("restarting")
                                            .json_data(json!({}))
                                            .unwrap_or_default()
                                    );
                                    break;
                                }
                                _ => {
                                    yield Ok(
                                        Event::default()
                                            .event("log")
                                            .json_data(json!({ "line": message }))
                                            .unwrap_or_default()
                                    );
                                }
                            }
                        } else {
                            yield Ok(
                                Event::default()
                                    .event("log")
                                    .json_data(json!({ "line": message }))
                                    .unwrap_or_default()
                            );
                        }
                    } else {
                        // Plain text progress message
                        yield Ok(
                            Event::default()
                                .event("log")
                                .json_data(json!({ "line": message }))
                                .unwrap_or_default()
                        );
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Self-update SSE lagged by {n} messages");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── UpdateCheckResponse serialization tests ───────────────────────────

    #[test]
    fn test_update_check_response_serialization() {
        let response = UpdateCheckResponse {
            update_available: true,
            current_version: "1.26.2".to_string(),
            latest_version: "1.27.0".to_string(),
            release_notes: "Bug fixes and improvements".to_string(),
            published_at: "2026-04-18T10:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: UpdateCheckResponse = serde_json::from_str(&json).unwrap();

        assert!(deserialized.update_available);
        assert_eq!(deserialized.current_version, "1.26.2");
        assert_eq!(deserialized.latest_version, "1.27.0");
    }

    #[test]
    fn test_update_check_response_no_update() {
        let response = UpdateCheckResponse {
            update_available: false,
            current_version: "1.26.2".to_string(),
            latest_version: "1.26.2".to_string(),
            release_notes: String::new(),
            published_at: String::new(),
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: UpdateCheckResponse = serde_json::from_str(&json).unwrap();

        assert!(!deserialized.update_available);
    }

    // ── UpdateTriggerResponse serialization tests ─────────────────────────

    #[test]
    fn test_update_trigger_response_serialization() {
        let response = UpdateTriggerResponse {
            ok: true,
            message: "Update started".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: UpdateTriggerResponse = serde_json::from_str(&json).unwrap();

        assert!(deserialized.ok);
        assert_eq!(deserialized.message, "Update started");
    }

    #[test]
    fn test_update_trigger_response_error() {
        let response = UpdateTriggerResponse {
            ok: false,
            message: "Update already in progress".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: UpdateTriggerResponse = serde_json::from_str(&json).unwrap();

        assert!(!deserialized.ok);
        assert_eq!(deserialized.message, "Update already in progress");
    }
}
