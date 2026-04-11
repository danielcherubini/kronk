use std::sync::Arc;

use async_stream;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{sse::Event, sse::KeepAlive, IntoResponse, Response, Sse},
    Json,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};

use super::types::{is_safe_path_component, QuantEntry};
use crate::db::queries;
use crate::gpu::VramInfo;
use crate::proxy::ProxyState;

/// Query parameters for the metrics history endpoint.
#[derive(Debug, Deserialize)]
pub struct HistoryQueryParams {
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    100
}

/// One historical metrics sample returned by the history endpoint.
#[derive(Debug, Serialize)]
pub struct MetricsHistoryEntry {
    pub ts_unix_ms: i64,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: i64,
    pub ram_total_mib: i64,
    pub gpu_utilization_pct: Option<i64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
}

impl From<queries::SystemMetricsRow> for MetricsHistoryEntry {
    fn from(row: queries::SystemMetricsRow) -> Self {
        MetricsHistoryEntry {
            ts_unix_ms: row.ts_unix_ms,
            cpu_usage_pct: row.cpu_usage_pct,
            ram_used_mib: row.ram_used_mib,
            ram_total_mib: row.ram_total_mib,
            gpu_utilization_pct: row.gpu_utilization_pct,
            vram_used_mib: row.vram_used_mib,
            vram_total_mib: row.vram_total_mib,
        }
    }
}

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

/// Handle system health check (Koji management API).
pub async fn handle_koji_system_health(
    state: State<Arc<ProxyState>>,
) -> Json<SystemHealthResponse> {
    let models_loaded = state.models.read().await.len();
    let metrics = state.system_metrics.read().await;

    Json(SystemHealthResponse {
        status: "ok",
        service: "koji",
        models_loaded,
        cpu_usage_pct: metrics.cpu_usage_pct,
        ram_used_mib: metrics.ram_used_mib,
        ram_total_mib: metrics.ram_total_mib,
        gpu_utilization_pct: metrics.gpu_utilization_pct,
        vram: metrics.vram.clone(),
    })
}

/// Handle listing available GGUF quants for a HuggingFace repo (Koji management API).
///
/// `repo_id` is captured as a wildcard path segment (e.g. `bartowski/Qwen3-8B-GGUF`)
/// because HF repo IDs contain a `/`. Registered as `GET /koji/v1/hf/*repo_id`.
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

/// Handle system restart (Koji management API).
/// Triggers a graceful shutdown and then exits the process.
pub async fn handle_koji_system_restart(state: State<Arc<ProxyState>>) -> Response {
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
        .body(axum::body::Body::from("Koji is shutting down"))
        .unwrap()
}

/// Stream live system metrics samples as SSE events.
///
/// Subscribes to the `metrics_tx` broadcast channel in `ProxyState`. Each
/// sample emitted by the metrics task (every 2s) is forwarded as an
/// `event: "sample"` SSE event with a JSON-serialized `MetricSample` body.
/// On subscriber lag, emits an `event: "lagged"` event with `{"missed": N}`
/// and continues. On channel close, the stream ends.
///
/// No historical backfill — the stream begins from the next live sample.
///
/// Registered as `GET /koji/v1/system/metrics/stream`.
pub async fn handle_system_metrics_stream(
    State(state): State<Arc<ProxyState>>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.metrics_tx.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(sample) => {
                    match serde_json::to_string(&sample) {
                        Ok(data) => yield Ok(Event::default().event("sample").data(data)),
                        Err(e) => tracing::warn!("failed to serialize MetricSample: {}", e),
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    let data = format!("{{\"missed\":{}}}", n);
                    yield Ok(Event::default().event("lagged").data(data));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Fetch historical system metrics from the database.
///
/// Returns up to `limit` most recent samples (oldest-first). If the database
/// is unavailable or contains no rows, returns an empty array (HTTP 200).
/// The `limit` parameter defaults to 100 and is clamped to 1–1000.
pub async fn handle_system_metrics_history(
    State(state): State<Arc<ProxyState>>,
    Query(params): Query<HistoryQueryParams>,
) -> Json<Vec<MetricsHistoryEntry>> {
    let limit = params.limit.clamp(1, 1000);

    let entries: Vec<MetricsHistoryEntry> = match state.open_db() {
        Some(conn) => match queries::get_recent_system_metrics(&conn, limit) {
            Ok(rows) => rows.into_iter().map(MetricsHistoryEntry::from).collect(),
            Err(e) => {
                tracing::warn!("failed to query metrics history: {}", e);
                vec![]
            }
        },
        None => vec![],
    };

    Json(entries)
}
