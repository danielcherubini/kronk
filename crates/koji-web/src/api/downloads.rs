//! Downloads Center API endpoints.
//!
//! Provides REST endpoints to query the download queue (active + history),
//! cancel items, and stream real-time events via SSE.

use async_stream::stream;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{
    sse::{Event, KeepAlive},
    Json, Sse,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::server::AppState;

// ── DTO types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadQueueItemDto {
    pub job_id: String,
    pub repo_id: String,
    pub filename: String,
    pub display_name: Option<String>,
    pub status: String,
    pub bytes_downloaded: i64,
    pub total_bytes: Option<i64>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub queued_at: String,
    pub kind: String,
    pub progress_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadsActiveResponse {
    pub items: Vec<DownloadQueueItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadsHistoryResponse {
    pub items: Vec<DownloadQueueItemDto>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadCancelResponse {
    pub ok: bool,
    pub message: Option<String>,
}

/// Convert a `koji_core::db::queries::DownloadQueueItem` to a DTO with computed progress.
fn item_to_dto(item: &koji_core::db::queries::DownloadQueueItem) -> DownloadQueueItemDto {
    let progress_percent = match item.total_bytes {
        Some(total) if total > 0 => (item.bytes_downloaded as f64 / total as f64) * 100.0,
        _ => 0.0,
    };

    DownloadQueueItemDto {
        job_id: item.job_id.clone(),
        repo_id: item.repo_id.clone(),
        filename: item.filename.clone(),
        display_name: item.display_name.clone(),
        status: item.status.clone(),
        bytes_downloaded: item.bytes_downloaded,
        total_bytes: item.total_bytes,
        error_message: item.error_message.clone(),
        started_at: item.started_at.clone(),
        completed_at: item.completed_at.clone(),
        queued_at: item.queued_at.clone(),
        kind: item.kind.clone(),
        progress_percent,
    }
}

// ── Query params for history endpoint ────────────────────────────────────────

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default = "default_offset")]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

fn default_offset() -> i64 {
    0
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// GET /api/downloads/active
pub async fn get_active_downloads(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DownloadsActiveResponse>, (StatusCode, Json<serde_json::Value>)> {
    let svc = state.download_queue.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "Download queue not configured"})),
        )
    })?;

    let items = svc.get_active_items().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let dto_items: Vec<DownloadQueueItemDto> = items.iter().map(item_to_dto).collect();

    Ok(Json(DownloadsActiveResponse { items: dto_items }))
}

/// GET /api/downloads/history?limit=50&offset=0
pub async fn get_download_history(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> Result<Json<DownloadsHistoryResponse>, (StatusCode, Json<serde_json::Value>)> {
    let svc = state.download_queue.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "Download queue not configured"})),
        )
    })?;

    let items = svc
        .get_history_items(query.limit, query.offset)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    let total = svc.count_history_items().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let dto_items: Vec<DownloadQueueItemDto> = items.iter().map(item_to_dto).collect();

    Ok(Json(DownloadsHistoryResponse {
        items: dto_items,
        total,
    }))
}

/// POST /api/downloads/:job_id/cancel
pub async fn cancel_download(
    State(state): State<Arc<AppState>>,
    Path(job_id): axum::extract::Path<String>,
) -> Json<DownloadCancelResponse> {
    let svc = match &state.download_queue {
        Some(svc) => svc,
        None => {
            return Json(DownloadCancelResponse {
                ok: false,
                message: Some("Download queue not configured".to_string()),
            })
        }
    };

    match svc.cancel(&job_id) {
        Ok(()) => Json(DownloadCancelResponse {
            ok: true,
            message: None,
        }),
        Err(e) => Json(DownloadCancelResponse {
            ok: false,
            message: Some(e.to_string()),
        }),
    }
}

/// GET /api/downloads/events — SSE stream of download lifecycle events.
pub async fn download_events_sse(
    State(state): State<Arc<AppState>>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
    let svc = state
        .download_queue
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let mut rx = svc.subscribe_events();

    let stream = stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let sse_event = match event {
                        koji_core::proxy::download_queue::DownloadEvent::Started { job_id, repo_id, filename, total_bytes } => {
                            Event::default()
                                .event("Started")
                                .json_data(serde_json::json!({
                                    "event": "Started",
                                    "job_id": job_id,
                                    "repo_id": repo_id,
                                    "filename": filename,
                                    "total_bytes": total_bytes,
                                }))
                        }
                        koji_core::proxy::download_queue::DownloadEvent::Progress { job_id, bytes_downloaded, total_bytes } => {
                            Event::default()
                                .event("Progress")
                                .json_data(serde_json::json!({
                                    "event": "Progress",
                                    "job_id": job_id,
                                    "bytes_downloaded": bytes_downloaded,
                                    "total_bytes": total_bytes,
                                }))
                        }
                        koji_core::proxy::download_queue::DownloadEvent::Verifying { job_id, filename } => {
                            Event::default()
                                .event("Verifying")
                                .json_data(serde_json::json!({
                                    "event": "Verifying",
                                    "job_id": job_id,
                                    "filename": filename,
                                }))
                        }
                        koji_core::proxy::download_queue::DownloadEvent::Completed { job_id, filename, size_bytes, duration_ms } => {
                            Event::default()
                                .event("Completed")
                                .json_data(serde_json::json!({
                                    "event": "Completed",
                                    "job_id": job_id,
                                    "filename": filename,
                                    "size_bytes": size_bytes,
                                    "duration_ms": duration_ms,
                                }))
                        }
                        koji_core::proxy::download_queue::DownloadEvent::Failed { job_id, filename, error } => {
                            Event::default()
                                .event("Failed")
                                .json_data(serde_json::json!({
                                    "event": "Failed",
                                    "job_id": job_id,
                                    "filename": filename,
                                    "error": error,
                                }))
                        }
                        koji_core::proxy::download_queue::DownloadEvent::Cancelled { job_id, filename } => {
                            Event::default()
                                .event("Cancelled")
                                .json_data(serde_json::json!({
                                    "event": "Cancelled",
                                    "job_id": job_id,
                                    "filename": filename,
                                }))
                        }
                        koji_core::proxy::download_queue::DownloadEvent::Queued { job_id, repo_id, filename } => {
                            Event::default()
                                .event("Queued")
                                .json_data(serde_json::json!({
                                    "job_id": job_id,
                                    "repo_id": repo_id,
                                    "filename": filename,
                                }))
                        }
                    };

                    match sse_event {
                        Ok(e) => yield Ok(e),
                        Err(e) => yield Err(axum::Error::new(e)),
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Client fell behind; emit a marker event with the lag count.
                    yield Ok(Event::default()
                        .event("Lagged")
                        .json_data(serde_json::json!({ "lagged": n }))?);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
