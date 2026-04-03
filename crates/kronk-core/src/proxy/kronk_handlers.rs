use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::proxy::{pull_jobs::PullJob, ProxyState};

/// Request body for pull job.
#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub repo_id: String,
    #[serde(default)]
    pub filename: Option<String>,
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
    // Get model card
    let model_card = state.get_model_card(&model_id).await;

    match model_card {
        Some(card) => {
            match state.load_model(&model_id, Some(&card)).await {
                Ok(_) => {
                    // Check if model is now loaded
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
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Model not configured",
                    "type": "NotFoundError"
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

/// Handle starting a pull job (Kronk management API).
pub async fn handle_kronk_pull_model(
    state: State<Arc<ProxyState>>,
    Json(request): Json<PullRequest>,
) -> Response {
    let job_id = format!("pull-{}", uuid::Uuid::new_v4().hyphenated());

    let repo_id = request.repo_id.clone();
    let filename = request.filename.clone().unwrap_or_else(|| {
        // Default filename handling would go here
        // For now, use a placeholder
        "default.gguf".to_string()
    });

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

    // Spawn download task
    tokio::spawn(async move {
        // Download logic would go here
        // For now, we'll just mark it as running
    });

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

/// Handle system health check (Kronk management API).
pub async fn handle_kronk_system_health(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let models_loaded = state.models.read().await.len();

    Json(serde_json::json!({
        "status": "ok",
        "service": "kronk",
        "models_loaded": models_loaded,
        "vram": {
            "used_mib": 0,
            "total_mib": 0
        }
    }))
}

/// Handle system restart (Kronk management API).
/// TODO: Implement actual restart logic using ProxyState methods
pub async fn handle_kronk_system_restart(_state: State<Arc<ProxyState>>) -> Response {
    Json(serde_json::json!({
        "message": "Restarting kronk..."
    }))
    .into_response()
}
