use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::jobs::JobKind;
use crate::server::AppState;
use koji_core::backends::{
    check_latest_version, BackendRegistry, BackendSource, BackendType, InstallOptions,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckDto {
    pub item_type: String, // "backend" or "model"
    pub item_id: String,   // backend name or model config key
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub status: String,
    pub error_message: Option<String>,
    pub details_json: Option<serde_json::Value>,
    pub checked_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdatesListResponse {
    pub backends: Vec<UpdateCheckDto>,
    pub models: Vec<UpdateCheckDto>,
}

#[derive(Debug, Serialize)]
pub struct CheckResponse {
    pub triggered: bool,
    pub message: String,
}

/// GET /api/updates - Returns cached results from DB
pub async fn get_updates(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "config_path not configured" })),
            )
                .into_response()
        }
    };

    let checker = koji_core::updates::UpdateChecker::new();
    match checker.get_results(&config_dir).await {
        Ok(records) => {
            let mut backends = Vec::new();
            let mut models = Vec::new();
            for r in records {
                let dto = UpdateCheckDto {
                    item_type: r.item_type,
                    item_id: r.item_id,
                    current_version: r.current_version,
                    latest_version: r.latest_version,
                    update_available: r.update_available,
                    status: r.status,
                    error_message: r.error_message,
                    details_json: r.details_json.and_then(|j| serde_json::from_str(&j).ok()),
                    checked_at: r.checked_at,
                };
                if dto.item_type == "backend" {
                    backends.push(dto);
                } else {
                    models.push(dto);
                }
            }
            Json(UpdatesListResponse { backends, models }).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /api/updates/check - Trigger full re-check
pub async fn trigger_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "config_path not configured" })),
            )
                .into_response()
        }
    };

    let checker = koji_core::updates::UpdateChecker::new();
    // Run in background, return immediately
    tokio::spawn(async move {
        if let Err(e) = checker.run_check(&config_dir).await {
            tracing::error!("Background update check failed: {}", e);
        }
    });

    Json(CheckResponse {
        triggered: true,
        message: "Update check started".to_string(),
    })
    .into_response()
}

/// POST /api/updates/check/:item_type/:item_id - Check single item
pub async fn check_single(
    State(state): State<Arc<AppState>>,
    Path((item_type, item_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "config_path not configured" })),
            )
                .into_response()
        }
    };

    let checker = koji_core::updates::UpdateChecker::new();
    let result = match item_type.as_str() {
        "backend" => {
            let config_dir_clone = config_dir.clone();
            let item_id_clone = item_id.clone();
            let bt_result =
                tokio::task::spawn_blocking(move || -> anyhow::Result<Option<BackendType>> {
                    let open = koji_core::db::open(&config_dir_clone)?;
                    let record =
                        koji_core::db::queries::get_active_backend(&open.conn, &item_id_clone)?;
                    Ok(record.map(|r| match r.backend_type.as_str() {
                        "llama_cpp" => BackendType::LlamaCpp,
                        "ik_llama" => BackendType::IkLlama,
                        _ => BackendType::Custom,
                    }))
                })
                .await;

            match bt_result {
                Ok(Ok(Some(bt))) => checker
                    .check_backend(&config_dir, &item_id, &bt)
                    .await
                    .map(|_| ()),
                Ok(Ok(None)) => Err(anyhow::anyhow!("Backend not found")),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(anyhow::anyhow!("Join error: {}", e)),
            }
        }
        "model" => {
            let config_dir_clone = config_dir.clone();
            let item_id_clone = item_id.clone();
            let rid_result =
                tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
                    let cfg = koji_core::config::Config::load_from(&config_dir_clone)?;
                    Ok(cfg.models.get(&item_id_clone).and_then(|m| m.model.clone()))
                })
                .await;

            match rid_result {
                Ok(Ok(rid)) => checker
                    .check_model(&config_dir, &item_id, rid.as_deref())
                    .await
                    .map(|_| ()),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(anyhow::anyhow!("Join error: {}", e)),
            }
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Invalid item_type" })),
            )
                .into_response()
        }
    };

    match result {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /api/updates/apply/backend/:name - Trigger backend update
pub async fn apply_backend_update(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "config_path not configured" })),
            )
                .into_response()
        }
    };

    // Load backend info from DB
    let bt_result = tokio::task::spawn_blocking({
        let config_dir = config_dir.clone();
        let name = name.clone();
        move || -> anyhow::Result<(Option<BackendType>, Option<String>)> {
            let open = koji_core::db::open(&config_dir)?;
            let record = koji_core::db::queries::get_active_backend(&open.conn, &name)?;
            Ok(record
                .map(|r| {
                    let bt = match r.backend_type.as_str() {
                        "llama_cpp" => BackendType::LlamaCpp,
                        "ik_llama" => BackendType::IkLlama,
                        _ => BackendType::Custom,
                    };
                    (Some(bt), Some(r.version))
                })
                .unwrap_or((None, None)))
        }
    })
    .await;

    let (backend_type, current_version) = match bt_result {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    let (Some(backend_type), Some(_version)) = (backend_type, current_version) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Backend not found" })),
        )
            .into_response();
    };

    let jobs = match &state.jobs {
        Some(j) => j.clone(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "job manager not configured" })),
            )
                .into_response()
        }
    };

    let job = match jobs
        .submit(crate::jobs::JobKind::Update, Some(backend_type.clone()))
        .await
    {
        Ok(j) => j,
        Err(crate::jobs::JobError::AlreadyRunning(existing_id)) => {
            return (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "another backend job is already running", "job_id": existing_id }))).into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "failed to create job" })),
            )
                .into_response()
        }
    };

    let latest_version = match check_latest_version(&backend_type).await {
        Ok(v) => v,
        Err(e) => return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Failed to check latest version: {}", e) })),
        )
            .into_response(),
    };

    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    let name_clone = name.clone();
    tokio::spawn(async move {
        let config_dir = koji_core::config::Config::base_dir().unwrap();
        let mut registry = BackendRegistry::open(&config_dir).unwrap();
        let backend_info = registry.get(&name_clone).unwrap().unwrap();

        let options = InstallOptions {
            backend_type: backend_type.clone(),
            source: backend_info
                .source
                .clone()
                .unwrap_or_else(|| BackendSource::SourceCode {
                    version: "main".to_string(),
                    git_url: "https://github.com/ggml-org/llama.cpp.git".to_string(),
                    commit: None,
                }),
            target_dir: backend_info.path.parent().unwrap().to_path_buf(),
            gpu_type: backend_info.gpu_type,
            allow_overwrite: true,
        };

        match koji_core::backends::update_backend_with_progress(
            &mut registry,
            &name_clone,
            options,
            latest_version,
            None,
        )
        .await
        {
            Ok(_) => {
                let _ = jobs_clone
                    .finish(&job_clone, crate::jobs::JobStatus::Succeeded, None)
                    .await;
            }
            Err(e) => {
                let _ = jobs_clone
                    .finish(
                        &job_clone,
                        crate::jobs::JobStatus::Failed,
                        Some(e.to_string()),
                    )
                    .await;
            }
        }
    });

    Json(serde_json::json!({ "job_id": job.id.to_string(), "kind": "update" })).into_response()
}

/// POST /api/updates/apply/model/:id - Trigger model re-pull (resolve model ID to repo_id, then trigger re-pull)
pub async fn apply_model_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "config_path not configured" })),
            )
                .into_response()
        }
    };

    let res_result = tokio::task::spawn_blocking({
        let config_dir = config_dir.clone();
        let id = id.clone();
        move || -> anyhow::Result<(String, std::path::PathBuf)> {
            let config = koji_core::config::Config::load_from(&config_dir)?;
            let model = config
                .models
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("Model not found"))?;
            let repo_id = model
                .model
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Model has no source"))?;
            let models_dir = config.models_dir()?;
            Ok((repo_id, models_dir))
        }
    })
    .await;

    let (repo_id, _models_dir) = match res_result {
        Ok(Ok(val)) => val,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    match koji_core::models::pull::list_gguf_files(&repo_id).await {
        Ok(listing) => {
            let res_result = tokio::task::spawn_blocking({
                let config_dir = config_dir.clone();
                let repo_id = repo_id.clone();
                let commit_sha = listing.commit_sha.clone();
                move || -> anyhow::Result<()> {
                    let open = koji_core::db::open(&config_dir)?;
                    koji_core::db::queries::upsert_model_pull(&open.conn, &repo_id, &commit_sha)?;
                    Ok(())
                }
            })
            .await;

            match res_result {
                Ok(Ok(_)) => {
                    Json(serde_json::json!({ "ok": true, "repo_id": repo_id, "commit_sha": listing.commit_sha })).into_response()
                }
                Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Failed to fetch updates: {}", e) })),
        )
            .into_response(),
    }
}
