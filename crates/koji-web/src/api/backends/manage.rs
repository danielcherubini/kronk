use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use super::types::*;
use crate::server::AppState;

/// POST /api/backends/:name/update
pub async fn update_backend(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let jobs = match &state.jobs {
        Some(j) => j,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "job manager not configured"})),
            )
                .into_response();
        }
    };

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

    // Open registry and get backend
    let registry_result: Result<koji_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            koji_core::backends::BackendRegistry::open(&config_dir)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    let mut registry = match registry_result {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to open registry: {}", e)})),
            )
                .into_response();
        }
    };

    let backend_info = match registry.get(&name) {
        Ok(Some(info)) => info,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": format!("Backend '{}' not found", name)})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to get backend: {}", e)})),
            )
                .into_response();
        }
    };

    let backend_type = backend_info.backend_type.clone();

    // Check latest version
    let latest_version = match koji_core::backends::check_latest_version(&backend_type).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(
                    serde_json::json!({"error": format!("Failed to check latest version: {}", e)}),
                ),
            )
                .into_response();
        }
    };

    // Submit job
    let job = match jobs
        .submit(crate::jobs::JobKind::Update, Some(backend_type.clone()))
        .await
    {
        Ok(j) => j,
        Err(crate::jobs::JobError::AlreadyRunning(existing_id)) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "another backend job is already running",
                    "job_id": existing_id
                })),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "failed to create job"})),
            )
                .into_response();
        }
    };

    // Anchor target_dir at backends_dir()/<name> so repeated updates overwrite
    // the same directory instead of nesting into the previous install's subdir.
    let target_dir = match koji_core::backends::backends_dir() {
        Ok(d) => d.join(&name),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to get backends dir: {}", e)})),
            )
                .into_response();
        }
    };

    // Build update options
    let options = koji_core::backends::InstallOptions {
        backend_type: backend_type.clone(),
        source: backend_info.source.clone().unwrap_or_else(|| {
            // Fallback: use source code if no source recorded
            koji_core::backends::BackendSource::SourceCode {
                version: "main".to_string(),
                git_url: match &backend_type {
                    koji_core::backends::BackendType::LlamaCpp => {
                        "https://github.com/ggml-org/llama.cpp.git"
                    }
                    koji_core::backends::BackendType::IkLlama => {
                        "https://github.com/ikawrakow/ik_llama.cpp.git"
                    }
                    other => {
                        tracing::warn!(
                            "No source URL configured for backend type {:?}, using llama.cpp fallback",
                            other
                        );
                        "https://github.com/ggml-org/llama.cpp.git"
                    }
                }
                .to_string(),
                commit: None,
            }
        }),
        target_dir,
        gpu_type: backend_info.gpu_type,
        allow_overwrite: true,
    };

    // Spawn the update task
    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    let name_clone = name.clone();
    let latest_version_clone = latest_version.clone();
    tokio::spawn(async move {
        let adapter = Arc::new(JobAdapter {
            jobs: jobs_clone.clone(),
            job: job_clone.clone(),
        });

        let result = match koji_core::backends::update_backend_with_progress(
            &mut registry,
            &name_clone,
            options,
            latest_version_clone,
            Some(adapter),
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string()),
        };

        match result {
            Ok(_) => {
                let _ = jobs_clone
                    .finish(&job_clone, crate::jobs::JobStatus::Succeeded, None)
                    .await;
            }
            Err(e) => {
                let _ = jobs_clone
                    .finish(&job_clone, crate::jobs::JobStatus::Failed, Some(e))
                    .await;
            }
        }
    });

    Json(InstallResponse {
        job_id: job.id.to_string(),
        kind: "update".to_string(),
        backend_type: format!("{}", backend_type),
        notices: vec![],
    })
    .into_response()
}

/// DELETE /api/backends/:name/versions/:version
pub async fn remove_backend_version(
    State(state): State<Arc<AppState>>,
    Path((name, version)): Path<(String, String)>,
) -> impl IntoResponse {
    // Validate path params (prevent path traversal)
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid backend name: path separators or traversal sequences not allowed"})),
        )
            .into_response();
    }
    if version.contains('/') || version.contains('\\') || version.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid version: path separators or traversal sequences not allowed"})),
        )
            .into_response();
    }

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

    // Open registry and get the specific version
    let config_dir_clone = config_dir.clone();
    let registry_result: Result<koji_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            koji_core::backends::BackendRegistry::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    let mut registry = match registry_result {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to open registry: {}", e)})),
            )
                .into_response();
        }
    };

    // Get the specific version record before deleting
    // Use list_all_versions and find the matching version (conn is private)
    let versions = match registry.list_all_versions(&name) {
        Ok(Some(v)) => v,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Backend '{}' version '{}' not found", name, version)
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to query backend: {}", e)})),
            )
                .into_response();
        }
    };

    let info = match versions.iter().find(|v| v.version == version) {
        Some(v) => v.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Backend '{}' version '{}' not found", name, version)
                })),
            )
                .into_response();
        }
    };

    // Delete files FIRST (before any DB changes)
    let info_to_remove = koji_core::backends::BackendInfo {
        name: info.name.clone(),
        backend_type: info.backend_type.clone(),
        version: info.version.clone(),
        path: std::path::PathBuf::from(&info.path),
        installed_at: info.installed_at,
        gpu_type: None,
        source: None,
    };

    // Check if a job is running for this backend
    if let Some(jobs) = &state.jobs {
        if let Some(active_job) = jobs.active().await {
            let active_type = active_job
                .backend_type
                .as_ref()
                .map(|b| b.to_string())
                .unwrap_or_default();
            if active_type == info.backend_type.to_string() {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "a job is currently running for this backend"
                    })),
                )
                    .into_response();
            }
        }
    }

    if info_to_remove.path.exists() {
        if let Err(e) = koji_core::backends::safe_remove_installation(&info_to_remove) {
            let err_msg = e.to_string();
            if err_msg.contains("outside the managed backends directory") {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "path is outside the managed backends directory; remove manually"
                    })),
                )
                    .into_response();
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to remove files: {}", e)})),
            )
                .into_response();
        }
    }

    // Remove from registry (DB only — activates another version if this was active)
    if let Err(e) = registry.remove_version(&name, &version) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to remove version from registry: {}", e)})),
        )
            .into_response();
    }

    // Clean up update_check record for this backend
    if let Ok(open) = koji_core::db::open(&config_dir) {
        let _ = koji_core::db::queries::delete_update_check(&open.conn, "backend", &name);
    }

    Json(DeleteResponse { removed: true }).into_response()
}

/// POST /api/backends/:name/activate
pub async fn activate_backend_version(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<ActivateRequest>,
) -> impl IntoResponse {
    // Validate name
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid backend name"})),
        )
            .into_response();
    }

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

    let config_dir_clone = config_dir.clone();
    let version_clone = req.version.clone();
    let name_clone = name.clone();
    let version_for_error = version_clone.clone();
    let registry_result: Result<(koji_core::backends::BackendRegistry, bool), _> =
        tokio::task::spawn_blocking(move || {
            let mut reg = koji_core::backends::BackendRegistry::open(&config_dir_clone)?;
            let activated = reg.activate(&name_clone, &version_clone)?;
            Ok((reg, activated))
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    match registry_result {
        Ok((_, activated)) => {
            if !activated {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": format!("Version '{}' not found for backend '{}'", version_for_error, name)
                    })),
                )
                    .into_response();
            }

            Json(ActivateResponse {
                version: req.version,
                is_active: true,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to activate: {}", e)})),
        )
            .into_response(),
    }
}

/// POST /api/backends/:name/default-args
/// Update default_args for a backend in config.toml
#[derive(Deserialize)]
pub struct UpdateDefaultArgsRequest {
    pub default_args: Vec<String>,
}

pub async fn update_backend_default_args(
    State(state): State<Arc<AppState>>,
    Path(backend_name): Path<String>,
    Json(req): Json<UpdateDefaultArgsRequest>,
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

    // Load config
    let mut config = match koji_core::config::Config::load_from(&config_dir) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to load config: {}", e)})),
            )
                .into_response();
        }
    };

    // Update default_args for the backend
    if let Some(backend) = config.backends.get_mut(&backend_name) {
        backend.default_args = req.default_args;
    } else {
        // Backend doesn't exist in config, create it
        config.backends.insert(
            backend_name.clone(),
            koji_core::config::BackendConfig {
                path: None,
                default_args: req.default_args,
                health_check_url: None,
                version: None,
            },
        );
    }

    // Clone config for proxy sync
    let config_clone = config.clone();

    // Save config
    match tokio::task::spawn_blocking(move || config.save_to(&config_dir)).await {
        Ok(Ok(())) => {
            // Sync proxy config if available
            if let Some(ref proxy_config) = state.proxy_config {
                let mut pc = proxy_config.write().await;
                *pc = config_clone;
            }
            Json(serde_json::json!({"success": true})).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to save config: {}", e)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Task failed: {}", e)})),
        )
            .into_response(),
    }
}
