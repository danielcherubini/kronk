use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use futures_util::stream::{self, Stream};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::gpu::VramInfo;
use crate::proxy::{
    pull_jobs::{PullJob, PullJobStatus},
    ProxyState,
};

/// A single quantisation variant available for a HuggingFace GGUF repo.
#[derive(Debug, Serialize)]
pub struct QuantEntry {
    pub filename: String,
    pub quant: Option<String>,
    pub size_bytes: Option<i64>,
}

/// A single quantisation variant to download (used in multi-quant wizard format).
#[derive(Debug, Deserialize, Clone)]
pub struct QuantDownloadSpec {
    pub filename: String,
    pub quant: Option<String>,
    pub context_length: Option<u32>,
}

/// Request body for pull job.
#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub repo_id: String,
    /// Quant to download, e.g. "Q4_K_M". Required — omitting returns a 422 with available quants.
    /// Legacy single-quant support (kept for backward compat).
    #[serde(default)]
    pub quant: Option<String>,
    /// New multi-quant wizard format: list of quants to download.
    #[serde(default)]
    pub quants: Vec<QuantDownloadSpec>,
    #[serde(default)]
    pub context_length: Option<u32>,
}

/// Response for a pull job.
#[derive(Debug, Serialize)]
pub struct PullResponse {
    pub job_id: String,
    pub status: String,
    pub repo_id: String,
    pub filename: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

/// Response for model load/unload.
#[derive(Debug, Serialize)]
pub struct ModelResponse {
    pub id: String,
    pub loaded: bool,
}

/// Response for system restart.
#[derive(Debug, Serialize)]
pub struct RestartResponse {
    pub message: String,
}

/// Handle listing all configured models (Kronk management API).
pub async fn handle_kronk_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let models = state.build_status_response().await;
    let models_obj = models.get("models").and_then(|v| v.as_object());

    let result: Vec<serde_json::Value> = models_obj
        .into_iter()
        .flat_map(|models_obj| {
            models_obj.iter().filter_map(|(id, model)| {
                model.as_object().and_then(|model| {
                    serde_json::to_value(model).ok().map(|mut m| {
                        m["id"] = serde_json::Value::String(id.clone());
                        m
                    })
                })
            })
        })
        .collect();

    Json(serde_json::json!({
        "models": result
    }))
}

/// Handle getting a single model's state (Kronk management API).
pub async fn handle_kronk_get_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    // Check if already loaded (by server name or model name)
    let model_state = state.get_model_state(&model_id).await;

    if let Some(ms) = model_state {
        let load_time = ms.load_time().unwrap_or(std::time::SystemTime::now());
        let owned_by = ms.backend();
        let created = load_time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO)
            .as_secs();
        return Json(serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": created,
            "owned_by": owned_by,
            "ready": ms.is_ready()
        }))
        .into_response();
    }

    // Check if it's a configured (but not loaded) model
    for (config_name, server_cfg) in &state.config.models {
        if !server_cfg.enabled {
            continue;
        }
        if config_name == &model_id || server_cfg.model.as_deref() == Some(model_id.as_str()) {
            return Json(serde_json::json!({
                "id": config_name,
                "object": "model",
                "created": 0,
                "owned_by": server_cfg.backend,
                "ready": false
            }))
            .into_response();
        }
    }

    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "message": "Model not found",
                "type": "NotFoundError"
            }
        })),
    )
        .into_response()
}

/// Handle loading a model (Kronk management API).
pub async fn handle_kronk_load_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    // Check the model is present in config (model card is optional)
    if !state.config.models.contains_key(&model_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Model not configured",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response();
    }

    // Model card is optional — pass None if it doesn't exist on disk
    let model_card = state.get_model_card(&model_id).await;

    match state.load_model(&model_id, model_card.as_ref()).await {
        Ok(_) => {
            let model_state = state.get_model_state(&model_id).await;
            let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
            Json(ModelResponse {
                id: model_id,
                loaded,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Failed to load model: {}", e),
                    "type": "LoadModelError"
                }
            })),
        )
            .into_response(),
    }
}

/// Handle unloading a model (Kronk management API).
pub async fn handle_kronk_unload_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    // Get the server name for this model
    let server_name = state.get_available_server_for_model(&model_id).await;

    match server_name {
        Some(server_name) => {
            // Unload the model
            match state.unload_model(&server_name).await {
                Ok(_) => {
                    let model_state = state.get_model_state(&model_id).await;
                    let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
                    Json(ModelResponse {
                        id: model_id,
                        loaded,
                    })
                    .into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Failed to unload model: {}", e),
                            "type": "UnloadModelError"
                        }
                    })),
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Model not configured or not loaded",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response(),
    }
}

/// Spawn a real download task for a single file and return the created `PullJob`.
///
/// The job is inserted into `pull_jobs` before this function returns.
fn spawn_download_job(
    pull_jobs_arc: Arc<tokio::sync::RwLock<std::collections::HashMap<String, PullJob>>>,
    job_id: String,
    repo_id: String,
    filename: String,
) {
    let job_id_clone = job_id.clone();
    let repo_id_clone = repo_id.clone();
    let filename_clone = filename.clone();

    tokio::spawn(async move {
        // Update status to Running
        {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Running;
                tracing::info!(job_id = %job_id_clone, "Job transitioned to Running");
            } else {
                tracing::warn!(job_id = %job_id_clone, "Job not found when setting Running");
                return;
            }
        }

        let config = match crate::config::Config::load() {
            Ok(c) => c,
            Err(e) => {
                let mut jobs = pull_jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                    job.error = Some(format!("Failed to load config: {}", e));
                }
                return;
            }
        };
        let models_dir = match config.models_dir() {
            Ok(d) => d,
            Err(e) => {
                let mut jobs = pull_jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                    job.error = Some(format!("Failed to get models dir: {}", e));
                }
                return;
            }
        };
        let repo_slug = repo_id_clone.replace('/', "--");
        let dest_dir = models_dir.join(&repo_slug);
        if let Err(e) = std::fs::create_dir_all(&dest_dir) {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some(format!("Failed to create dest dir: {}", e));
            }
            return;
        }

        let dest_path = dest_dir.join(&filename_clone);
        let url = format!(
            "https://huggingface.co/{}/resolve/main/{}",
            repo_id_clone, filename_clone
        );

        // HEAD request to get total_bytes upfront
        let client = reqwest::Client::new();
        if let Ok(resp) = client.head(&url).send().await {
            let total = crate::models::download::parse_content_length(resp.headers());
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.total_bytes = total;
            }
        }

        // Real download
        match crate::models::download::download_chunked(&url, &dest_path, 8, None).await {
            Ok(bytes) => {
                let mut jobs = pull_jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.bytes_downloaded = bytes;
                    job.total_bytes = Some(bytes);
                    job.status = crate::proxy::pull_jobs::PullJobStatus::Completed;
                    job.completed_at = Some(std::time::Instant::now());
                    tracing::info!(job_id = %job_id_clone, bytes, "Job transitioned to Completed");
                }
            }
            Err(e) => {
                let mut jobs = pull_jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                    job.error = Some(e.to_string());
                    tracing::error!(job_id = %job_id_clone, error = %e, "Job failed");
                }
            }
        }
    });
}

/// Handle starting a pull job (Kronk management API).
pub async fn handle_kronk_pull_model(
    state: State<Arc<ProxyState>>,
    Json(request): Json<PullRequest>,
) -> Response {
    let repo_id = request.repo_id.clone();

    // Multi-quant path: when `quants` is non-empty, spawn one job per entry.
    if !request.quants.is_empty() {
        let pull_jobs_arc = Arc::clone(&state.pull_jobs);
        let mut job_entries = Vec::with_capacity(request.quants.len());

        for spec in &request.quants {
            let job_id = format!("pull-{}", uuid::Uuid::new_v4().hyphenated());
            let pull_job = PullJob {
                job_id: job_id.clone(),
                repo_id: repo_id.clone(),
                filename: spec.filename.clone(),
                status: crate::proxy::pull_jobs::PullJobStatus::Pending,
                bytes_downloaded: 0,
                total_bytes: None,
                error: None,
                completed_at: None,
            };

            {
                let mut jobs = pull_jobs_arc.write().await;
                jobs.insert(job_id.clone(), pull_job);
            }

            spawn_download_job(
                Arc::clone(&pull_jobs_arc),
                job_id.clone(),
                repo_id.clone(),
                spec.filename.clone(),
            );

            job_entries.push(serde_json::json!({
                "job_id": job_id,
                "filename": spec.filename,
                "status": "pending"
            }));
        }

        return Json(serde_json::Value::Array(job_entries)).into_response();
    }

    // Legacy single-quant path.

    // Quant is required — if missing, fetch the available quants from HF and return them.
    let quant = match request.quant {
        Some(q) => q,
        None => {
            let available = match crate::models::pull::list_gguf_files(&repo_id).await {
                Ok(listing) => listing
                    .files
                    .into_iter()
                    .map(|f| {
                        serde_json::json!({
                            "filename": f.filename,
                            "quant": f.quant
                        })
                    })
                    .collect::<Vec<_>>(),
                Err(e) => {
                    tracing::warn!(repo_id = %repo_id, "Failed to fetch quant list: {}", e);
                    vec![]
                }
            };

            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": {
                        "message": "quant is required",
                        "type": "ValidationError",
                        "available_quants": available
                    }
                })),
            )
                .into_response();
        }
    };

    // Resolve the quant to a concrete filename from the HF listing.
    let listing = match crate::models::pull::list_gguf_files(&repo_id).await {
        Ok(l) => l,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Failed to fetch file list from HuggingFace: {}", e),
                        "type": "UpstreamError"
                    }
                })),
            )
                .into_response();
        }
    };

    // Find a file matching the requested quant (case-insensitive).
    let matched_file = listing
        .files
        .iter()
        .find(|f| f.quant.as_deref().map(|q| q.eq_ignore_ascii_case(&quant)) == Some(true));

    let filename = match matched_file {
        Some(f) => f.filename.clone(),
        None => {
            let available: Vec<serde_json::Value> = listing
                .files
                .into_iter()
                .map(|f| serde_json::json!({ "filename": f.filename, "quant": f.quant }))
                .collect();
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Quant '{}' not found in repo '{}'", quant, repo_id),
                        "type": "ValidationError",
                        "available_quants": available
                    }
                })),
            )
                .into_response();
        }
    };

    let job_id = format!("pull-{}", uuid::Uuid::new_v4().hyphenated());

    // Create pull job
    let pull_job = PullJob {
        job_id: job_id.clone(),
        repo_id: repo_id.clone(),
        filename: filename.clone(),
        status: crate::proxy::pull_jobs::PullJobStatus::Pending,
        bytes_downloaded: 0,
        total_bytes: None,
        error: None,
        completed_at: None,
    };

    // Store the job
    {
        let mut jobs = state.pull_jobs.write().await;
        jobs.insert(job_id.clone(), pull_job);
    }

    // Spawn real download task
    spawn_download_job(
        Arc::clone(&state.pull_jobs),
        job_id.clone(),
        repo_id.clone(),
        filename.clone(),
    );

    Json(serde_json::json!({
        "job_id": job_id,
        "status": "pending",
        "repo_id": repo_id,
        "filename": filename,
        "bytes_downloaded": 0,
        "total_bytes": null,
        "error": null
    }))
    .into_response()
}

/// Handle getting pull job status (Kronk management API).
pub async fn handle_kronk_get_pull_job(
    state: State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> Response {
    let jobs = state.pull_jobs.read().await;
    let job = jobs.get(&job_id).cloned();

    match job {
        Some(j) => {
            let status_str = match j.status {
                crate::proxy::pull_jobs::PullJobStatus::Pending => "pending",
                crate::proxy::pull_jobs::PullJobStatus::Running => "running",
                crate::proxy::pull_jobs::PullJobStatus::Completed => "completed",
                crate::proxy::pull_jobs::PullJobStatus::Failed => "failed",
            };

            Json(serde_json::json!({
                "job_id": j.job_id,
                "status": status_str,
                "repo_id": j.repo_id,
                "filename": j.filename,
                "bytes_downloaded": j.bytes_downloaded,
                "total_bytes": j.total_bytes,
                "error": j.error
            }))
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Pull job not found",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response(),
    }
}

/// Stream `PullJob` snapshots as SSE events every 500 ms until the job reaches a terminal state.
///
/// Events:
/// - `progress`: emitted while the job is pending or running
/// - `done`: emitted once when the job completes or fails, then the stream closes
///
/// Registered as `GET /kronk/v1/pulls/:job_id/stream`.
pub async fn handle_pull_job_stream(
    state: State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // State tuple: (proxy_state, job_id, just_emitted_done)
    let stream = stream::unfold(
        (state.0, job_id, false),
        |(state, job_id, just_done)| async move {
            // Previous iteration already emitted the done event — close the stream.
            if just_done {
                return None;
            }

            // Poll every 500 ms.
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            let jobs = state.pull_jobs.read().await;
            let Some(job) = jobs.get(&job_id).cloned() else {
                // Job not found — close the stream.
                return None;
            };
            drop(jobs);

            let is_terminal =
                matches!(job.status, PullJobStatus::Completed | PullJobStatus::Failed);
            let event_name = if is_terminal { "done" } else { "progress" };
            let data = serde_json::to_string(&job).unwrap_or_default();
            let event = Event::default().event(event_name).data(data);

            // If terminal, set just_done=true so the next iteration closes the stream.
            Some((Ok(event), (state, job_id, is_terminal)))
        },
    );

    Sse::new(stream).keep_alive(KeepAlive::default())
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

/// Handle system health check (Kronk management API).
pub async fn handle_kronk_system_health(
    state: State<Arc<ProxyState>>,
) -> Json<SystemHealthResponse> {
    let models_loaded = state.models.read().await.len();
    let metrics = state.system_metrics.read().await;

    Json(SystemHealthResponse {
        status: "ok",
        service: "kronk",
        models_loaded,
        cpu_usage_pct: metrics.cpu_usage_pct,
        ram_used_mib: metrics.ram_used_mib,
        ram_total_mib: metrics.ram_total_mib,
        gpu_utilization_pct: metrics.gpu_utilization_pct,
        vram: metrics.vram.clone(),
    })
}

/// Handle listing available GGUF quants for a HuggingFace repo (Kronk management API).
///
/// `repo_id` is captured as a wildcard path segment (e.g. `bartowski/Qwen3-8B-GGUF`)
/// because HF repo IDs contain a `/`. Registered as `GET /kronk/v1/hf/*repo_id`.
pub async fn handle_hf_list_quants(Path(repo_id): Path<String>) -> Response {
    match crate::models::pull::fetch_blob_metadata(&repo_id).await {
        Ok(blobs) => {
            let mut quants: Vec<QuantEntry> = blobs
                .into_values()
                .map(|b| QuantEntry {
                    quant: crate::models::pull::infer_quant_from_filename(&b.filename),
                    filename: b.filename,
                    size_bytes: b.size,
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

/// Handle system restart (Kronk management API).
/// TODO: Implement actual restart logic using ProxyState methods
pub async fn handle_kronk_system_restart(_state: State<Arc<ProxyState>>) -> Response {
    Json(serde_json::json!({
        "message": "Restarting kronk..."
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::pull_jobs::{PullJob, PullJobStatus};

    /// Verifies that `PullJob` serializes to JSON with the fields expected for SSE data.
    #[test]
    fn test_pull_job_serializes_for_sse() {
        let job = PullJob {
            job_id: "pull-test-123".to_string(),
            repo_id: "bartowski/Qwen3-8B-GGUF".to_string(),
            filename: "Qwen3-8B-Q4_K_M.gguf".to_string(),
            status: PullJobStatus::Running,
            bytes_downloaded: 1_234_567,
            total_bytes: Some(4_800_000_000),
            error: None,
            completed_at: None,
        };

        let json = serde_json::to_string(&job).expect("PullJob serialization failed");
        assert!(
            json.contains("\"bytes_downloaded\""),
            "missing bytes_downloaded in: {json}"
        );
        assert!(json.contains("\"status\""), "missing status in: {json}");
        assert!(
            json.contains("\"running\""),
            "missing running status value in: {json}"
        );
        assert!(json.contains("\"job_id\""), "missing job_id in: {json}");
    }

    /// Verifies that `QuantEntry` serializes to JSON with all expected keys.
    #[test]
    fn test_quant_entry_serializes() {
        let entry = QuantEntry {
            filename: "Model-Q4_K_M.gguf".to_string(),
            quant: Some("Q4_K_M".to_string()),
            size_bytes: Some(4_200_000_000),
        };

        let value = serde_json::to_value(&entry).expect("serialization failed");
        assert!(value.get("filename").is_some(), "missing filename");
        assert!(value.get("quant").is_some(), "missing quant");
        assert!(value.get("size_bytes").is_some(), "missing size_bytes");
        assert_eq!(value["filename"], "Model-Q4_K_M.gguf");
        assert_eq!(value["quant"], "Q4_K_M");
        assert_eq!(value["size_bytes"], 4_200_000_000_i64);
    }

    /// Verifies that `SystemHealthResponse` serializes to JSON with all expected fields.
    #[test]
    fn test_system_health_response_serializes() {
        let response = SystemHealthResponse {
            status: "ok",
            service: "kronk",
            models_loaded: 2,
            cpu_usage_pct: 42.5,
            ram_used_mib: 1024,
            ram_total_mib: 8192,
            gpu_utilization_pct: Some(75),
            vram: Some(crate::gpu::VramInfo {
                used_mib: 4000,
                total_mib: 8000,
            }),
        };

        let value = serde_json::to_value(&response).expect("serialization failed");
        assert!(
            value.get("cpu_usage_pct").is_some(),
            "missing cpu_usage_pct"
        );
        assert!(value.get("ram_used_mib").is_some(), "missing ram_used_mib");
        assert!(
            value.get("ram_total_mib").is_some(),
            "missing ram_total_mib"
        );
        assert!(
            value.get("gpu_utilization_pct").is_some(),
            "missing gpu_utilization_pct"
        );
        assert!(value.get("vram").is_some(), "missing vram");
    }
}
