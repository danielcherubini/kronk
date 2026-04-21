use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::*;
use crate::server::AppState;

/// POST /koji/v1/backends/install
pub async fn install_backend(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InstallRequest>,
) -> impl IntoResponse {
    // Validate backend_type: non-empty and <= 64 chars
    if req.backend_type.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "backend_type cannot be empty"})),
        )
            .into_response();
    }
    if req.backend_type.len() > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "backend_type must be at most 64 characters"})),
        )
            .into_response();
    }

    // Validate version: if provided, must be non-empty and <= 128 chars
    if let Some(ref version) = req.version {
        if version.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "version cannot be empty"})),
            )
                .into_response();
        }
        if version.len() > 128 {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "version must be at most 128 characters"})),
            )
                .into_response();
        }
    }

    // Validate gpu_type version fields: if present, must be non-empty and <= 32 chars
    match &req.gpu_type {
        GpuTypeDto::Cuda { version } | GpuTypeDto::Rocm { version } => {
            if version.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "gpu type version cannot be empty"})),
                )
                    .into_response();
            }
            if version.len() > 32 {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "gpu type version must be at most 32 characters"})),
                )
                    .into_response();
            }
        }
        _ => {}
    }

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

    // Parse backend type
    let backend_type = match req.backend_type.as_str() {
        "llama_cpp" => koji_core::backends::BackendType::LlamaCpp,
        "ik_llama" => koji_core::backends::BackendType::IkLlama,
        "custom" => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Custom backends cannot be installed via API"})),
            )
                .into_response();
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Unknown backend type: {}", req.backend_type)})),
            )
                .into_response();
        }
    };

    // Convert GPU type
    let gpu_type = match &req.gpu_type {
        GpuTypeDto::Cuda { version } => Some(koji_core::gpu::GpuType::Cuda {
            version: version.clone(),
        }),
        GpuTypeDto::Vulkan => Some(koji_core::gpu::GpuType::Vulkan),
        GpuTypeDto::Metal => Some(koji_core::gpu::GpuType::Metal),
        GpuTypeDto::Rocm { version } => Some(koji_core::gpu::GpuType::RocM {
            version: version.clone(),
        }),
        GpuTypeDto::CpuOnly => Some(koji_core::gpu::GpuType::CpuOnly),
        GpuTypeDto::Custom => Some(koji_core::gpu::GpuType::Custom),
    };

    // Compute effective build_from_source
    let is_linux = std::env::consts::OS == "linux";
    let is_cuda = matches!(&req.gpu_type, GpuTypeDto::Cuda { .. });
    let is_ik_llama = matches!(backend_type, koji_core::backends::BackendType::IkLlama);

    let mut notices: Vec<String> = Vec::new();
    let effective_build_from_source = if is_ik_llama {
        notices.push("ik_llama always builds from source".to_string());
        true
    } else if is_linux && is_cuda {
        notices.push("no prebuilt CUDA binary for Linux; building from source".to_string());
        true
    } else {
        req.build_from_source
    };

    // Check prerequisites if source build
    if effective_build_from_source {
        let cache = match &state.capabilities {
            Some(c) => c,
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "capabilities cache not configured"})),
                )
                    .into_response();
            }
        };

        let caps = match cache
            .get_or_compute(
                koji_core::gpu::detect_build_prerequisites,
                koji_core::gpu::detect_cuda_version,
            )
            .await
        {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("Capability detection failed: {}", e) })),
                )
                    .into_response();
            }
        };

        if !caps.git_available {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing build prerequisite: git"})),
            )
                .into_response();
        }
        if !caps.cmake_available {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing build prerequisite: cmake"})),
            )
                .into_response();
        }
        if !caps.compiler_available {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing build prerequisite: compiler"})),
            )
                .into_response();
        }
    }

    // Submit job
    let job = match jobs
        .submit(crate::jobs::JobKind::Install, Some(backend_type.clone()))
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

    // Build install options
    let version = req.version.unwrap_or_else(|| "latest".to_string());
    let git_url = match backend_type {
        koji_core::backends::BackendType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
        koji_core::backends::BackendType::IkLlama => {
            "https://github.com/ikawrakow/ik_llama.cpp.git"
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Unsupported backend type: {}", backend_type)})),
            )
                .into_response();
        }
    };

    let source = if effective_build_from_source {
        koji_core::backends::BackendSource::SourceCode {
            version: version.clone(),
            git_url: git_url.to_string(),
            commit: None,
        }
    } else {
        koji_core::backends::BackendSource::Prebuilt {
            version: version.clone(),
        }
    };

    let target_dir = match koji_core::backends::backends_dir() {
        Ok(d) => d.join(match backend_type {
            koji_core::backends::BackendType::LlamaCpp => "llama_cpp",
            koji_core::backends::BackendType::IkLlama => "ik_llama",
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": format!("Unsupported backend type: {}", backend_type)})),
                )
                    .into_response();
            }
        }),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to get backends dir: {}", e)})),
            )
                .into_response();
        }
    };

    // Capture values needed for DB registration before gpu_type/source are moved
    let reg_backend_type = backend_type.clone();
    let reg_version = version.clone();
    let reg_gpu_type = gpu_type.clone();
    let reg_source = source.clone();
    let reg_backend_name = match backend_type {
        koji_core::backends::BackendType::LlamaCpp => "llama_cpp",
        koji_core::backends::BackendType::IkLlama => "ik_llama",
        _ => "custom",
    }
    .to_string();
    let reg_config_dir = state
        .config_path
        .as_ref()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let options = koji_core::backends::InstallOptions {
        backend_type: backend_type.clone(),
        source,
        target_dir,
        gpu_type,
        allow_overwrite: req.force,
    };

    // Spawn the install task
    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    tokio::spawn(async move {
        let adapter = Arc::new(JobAdapter {
            jobs: jobs_clone.clone(),
            job: job_clone.clone(),
        });

        let result = match koji_core::backends::installer::install_backend_with_progress(
            options,
            Some(adapter),
            None, // No registry client available in background job
        )
        .await
        {
            Ok(binary_path) => Ok(binary_path),
            Err(e) => Err(e.to_string()),
        };

        match result {
            Ok(binary_path) => {
                // Register the installation in the DB so `resolve_backend_path` can find it.
                if let Some(config_dir) = reg_config_dir {
                    let installed_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let reg_result = tokio::task::spawn_blocking(move || {
                        let mut registry = koji_core::backends::BackendRegistry::open(&config_dir)?;
                        registry.add(koji_core::backends::BackendInfo {
                            name: reg_backend_name,
                            backend_type: reg_backend_type,
                            version: reg_version,
                            path: binary_path,
                            installed_at,
                            gpu_type: reg_gpu_type,
                            source: Some(reg_source),
                        })
                    })
                    .await;
                    if let Err(e) = reg_result {
                        tracing::warn!("Failed to register backend in DB: {}", e);
                    }
                }
                let _ = jobs_clone
                    .finish(&job_clone, crate::jobs::JobStatus::Succeeded, None)
                    .await;
            }
            Err(e) => {
                // Emit the error as a log line so it appears in the build log panel.
                jobs_clone
                    .append_log(&job_clone, format!("Error: {}", e))
                    .await;
                let _ = jobs_clone
                    .finish(&job_clone, crate::jobs::JobStatus::Failed, Some(e))
                    .await;
            }
        }
    });

    Json(InstallResponse {
        job_id: job.id.to_string(),
        kind: "install".to_string(),
        backend_type: req.backend_type,
        notices,
    })
    .into_response()
}

/// DELETE /koji/v1/backends/:name
pub async fn remove_backend(
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
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Invalid backend name: path separators or traversal sequences not allowed"
            })),
        )
            .into_response();
    }

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

    // Check if a job is running for this backend
    if let Some(active_job) = jobs.active().await {
        let active_type = active_job
            .backend_type
            .as_ref()
            .map(|b| b.to_string())
            .unwrap_or_default();
        if active_type == backend_info.backend_type.to_string() {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "a job is currently running for this backend"
                })),
            )
                .into_response();
        }
    }

    // Remove files
    if let Err(e) = koji_core::backends::safe_remove_installation(&backend_info) {
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

    // Remove from registry
    if let Err(e) = registry.remove(&name) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to remove from registry: {}", e)})),
        )
            .into_response();
    }

    // Clean up update_check record
    if let Ok(open) = koji_core::db::open(&config_dir) {
        let _ = koji_core::db::queries::delete_update_check(&open.conn, "backend", &name);
    }

    Json(DeleteResponse { removed: true }).into_response()
}
