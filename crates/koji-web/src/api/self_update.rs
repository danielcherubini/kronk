//! Self-update API endpoints for the Koji web UI.
//!
//! Provides three endpoints:
//! - `GET /api/self-update/check` — check if a new version is available
//! - `POST /api/self-update/update` — trigger the update (CSRF-protected)
//! - `GET /api/self-update/events` — SSE stream of update progress

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

/// Response for `GET /api/self-update/check`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResponse {
    pub update_available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: String,
    pub published_at: String,
}

/// Response for `POST /api/self-update/update`.
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
/// `GET /api/self-update/events` (SSE).
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
        let progress_callback = move |msg: String| {
            let _ = tx_clone.send(msg);
        };

        match koji_core::self_update::perform_update(&binary_version, progress_callback).await {
            Ok(result) => {
                let _ = tx.send(
                    json!({
                        "type": "status",
                        "status": "succeeded",
                        "old_version": result.old_version,
                        "new_version": result.new_version,
                    })
                    .to_string(),
                );

                let _ = tx.send(json!({"type": "restarting"}).to_string());

                // Brief delay before restarting to allow clients to receive the events
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                // Restart the process — this call does not return on success
                if let Err(e) = koji_core::self_update::restart_process() {
                    tracing::error!("Failed to restart after update: {e}");
                    let _ = tx.send(
                        json!({
                            "type": "status",
                            "status": "failed",
                            "error": format!("Update succeeded but restart failed: {e}")
                        })
                        .to_string(),
                    );
                }
            }
            Err(e) => {
                tracing::error!("Self-update failed: {e:#}");
                let _ = tx.send(
                    json!({
                        "type": "status",
                        "status": "failed",
                        "error": format!("{e:#}")
                    })
                    .to_string(),
                );
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
