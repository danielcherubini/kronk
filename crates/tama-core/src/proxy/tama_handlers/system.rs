use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event, sse::KeepAlive, IntoResponse, Response, Sse},
    Json,
};
use futures_util::Stream;

use serde::Serialize;

use super::types::{is_safe_path_component, QuantEntry};
use crate::gpu::VramInfo;
use crate::proxy::ProxyState;

/// Typed response for the system health endpoint.
#[derive(Debug, Serialize)]
pub struct SystemHealthResponse {
    pub status: &'static str,
    pub service: &'static str,
    pub models_loaded: usize,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    pub gpu_utilization_pct: Option<u8>,
    pub vram: Option<VramInfo>,
}

/// Handle system health check (Tama management API).
pub async fn handle_tama_system_health(
    state: State<Arc<ProxyState>>,
) -> Json<SystemHealthResponse> {
    let models_loaded = state.models.read().await.len();
    let metrics = state.system_metrics.read().await;

    Json(SystemHealthResponse {
        status: "ok",
        service: "tama",
        models_loaded,
        cpu_usage_pct: metrics.cpu_usage_pct,
        ram_used_mib: metrics.ram_used_mib,
        ram_total_mib: metrics.ram_total_mib,
        gpu_utilization_pct: metrics.gpu_utilization_pct,
        vram: metrics.vram.clone(),
    })
}

/// Handle listing available GGUF quants for a HuggingFace repo (Tama management API).
///
/// `repo_id` is captured as a wildcard path segment (e.g. `bartowski/Qwen3-8B-GGUF`)
/// because HF repo IDs contain a `/`. Registered as `GET /tama/v1/hf/*repo_id`.
pub async fn handle_hf_list_quants(Path(repo_id): Path<String>) -> Response {
    // Reject repo_id segments containing traversal sequences or null bytes (SSRF mitigation).
    if !repo_id.split('/').all(is_safe_path_component) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid repo_id" })),
        )
            .into_response();
    }

    match crate::models::pull::fetch_blob_metadata(&repo_id).await {
        Ok(blobs) => {
            let mut quants: Vec<QuantEntry> = blobs
                .into_values()
                .map(|b| {
                    let kind = crate::config::QuantKind::from_filename(&b.filename);
                    QuantEntry {
                        quant: crate::models::pull::infer_quant_from_filename(&b.filename),
                        filename: b.filename,
                        size_bytes: b.size,
                        kind,
                    }
                })
                .collect();
            quants.sort_by(|a, b| a.filename.cmp(&b.filename));
            (StatusCode::OK, Json(quants)).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Handle system restart (Tama management API).
/// Triggers a graceful shutdown and then exits the process.
pub async fn handle_tama_system_restart(state: State<Arc<ProxyState>>) -> Response {
    // Trigger graceful shutdown first
    state.0.shutdown().await;

    // Schedule process exit on a short delay so the HTTP response can be delivered.
    // We use std::process::exit(0) here because this is a hard restart operation
    // - we want to immediately terminate all background tasks (metrics, DB, etc.)
    // without waiting for them to drain. The shutdown() call above has already
    // cleared in-memory state (models, pull jobs, metrics channel).
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        std::process::exit(0);
    });

    // Return a response to the client
    Response::builder()
        .status(200)
        .body(axum::body::Body::from("Tama is shutting down"))
        .unwrap()
}

/// Stream live system metrics snapshots as SSE events.
///
/// Subscribes to the `metrics_tx` broadcast channel in `ProxyState`. Each
/// tick (every 2s), the metrics task broadcasts an `Arc<[MetricSample]>`
/// containing the full history buffer. This handler serializes the array
/// as JSON and emits it as `event: "snapshot"`.
///
/// On subscriber lag, the handler silently skips the missed tick — the next
/// snapshot will contain the full history. On channel close (empty Arc
/// sentinel), the stream ends.
///
/// Registered as `GET /tama/v1/system/metrics/stream`.
pub async fn handle_system_metrics_stream(
    State(state): State<Arc<ProxyState>>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.metrics_tx.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(samples) => {
                    if samples.is_empty() { break; } // Shutdown sentinel
                    match serde_json::to_string(samples.as_ref()) {
                        Ok(data) => yield Ok(Event::default().event("snapshot").data(data)),
                        Err(e) => tracing::warn!("failed to serialize MetricSample slice: {}", e),
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Subscriber lagged — next snapshot will have full history, no action needed
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}
