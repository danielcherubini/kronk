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

/// Query params for POST /tama/v1/backends/:name/update
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// POST /tama/v1/backends/:name/update
pub async fn update_backend(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<UpdateQuery>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid backend name: path separators or traversal sequences not allowed"})),
        )
            .into_response();
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
    let registry_result: Result<tama_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            tama_core::backends::BackendRegistry::open(&config_dir)
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

    // Determine gpu_variant: use explicit value or auto-infer from registry
    let lookup_variant = match query.gpu_variant {
        Some(v) => v,
        None => {
            // Auto-infer: find unique variant for this backend
            let versions = match registry.list_all_versions(&name, None) {
                Ok(Some(v)) => v,
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
                        Json(
                            serde_json::json!({"error": format!("Failed to query backend: {}", e)}),
                        ),
                    )
                        .into_response();
                }
            };
            let mut variants: Vec<String> =
                versions.iter().map(|v| v.gpu_variant.clone()).collect();
            variants.sort();
            variants.dedup();
            match variants.len() {
                1 => variants.into_iter().next().unwrap(),
                _ => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": format!(
                                "Backend '{}' has multiple variants. Please specify gpu_variant. Available: {}",
                                name,
                                variants.join(", ")
                            )
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    let backend_info = match registry.get(&name, &lookup_variant) {
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
    let latest_version = match tama_core::backends::check_latest_version(&backend_type).await {
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

    // Use versioned path structure for the update target
    let target_dir = match tama_core::backends::backends_dir() {
        Ok(d) => tama_core::backends::get_backend_install_path(
            &d,
            &backend_type,
            &backend_info.gpu_variant,
            &latest_version,
        ),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to get backends dir: {}", e)})),
            )
                .into_response();
        }
    };

    // Build update options
    let options = tama_core::backends::InstallOptions {
        backend_type: backend_type.clone(),
        source: backend_info.source.clone().unwrap_or_else(|| {
            // Fallback: use source code if no source recorded
            tama_core::backends::BackendSource::SourceCode {
                version: "main".to_string(),
                git_url: match &backend_type {
                    tama_core::backends::BackendType::LlamaCpp => {
                        "https://github.com/ggml-org/llama.cpp.git"
                    }
                    tama_core::backends::BackendType::IkLlama => {
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
        gpu_variant: backend_info.gpu_variant.clone(),
        allow_overwrite: true,
    };

    // Spawn the update task
    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    let name_clone = name.clone();
    let latest_version_clone = latest_version.clone();
    let gpu_variant_clone = backend_info.gpu_variant.clone();
    tokio::spawn(async move {
        let adapter = Arc::new(JobAdapter {
            jobs: jobs_clone.clone(),
            job: job_clone.clone(),
        });

        let result = match tama_core::backends::update_backend_with_progress(
            &mut registry,
            &name_clone,
            &gpu_variant_clone,
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// Path traversal in update_backend name should return 400.
    #[tokio::test]
    async fn test_update_backend_path_traversal_rejected() {
        let state = Arc::new(crate::server::AppState {
            jobs: None,
            capabilities: None,
            proxy_base_url: "http://127.0.0.1:11434".to_string(),
            client: reqwest::Client::new(),
            logs_dir: None,
            config_path: None,
            proxy_config: None,
            binary_version: "0.0.0-test".to_string(),
            update_tx: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            update_checker: Arc::new(tama_core::updates::UpdateChecker::new()),
            download_queue: None,
        });

        let router = crate::server::build_router(state);

        // Valid CSRF token pair — cookie and header must match.
        let csrf_token = "test-csrf-token-12345";
        let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

        // Test with `\` in name — backslash won't be normalized by Axum.
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends/foo\\bar/update")
            .header(axum::http::header::COOKIE, cookie_header.as_str())
            .header("X-CSRF-Token", csrf_token)
            .body(Body::empty())
            .unwrap();

        let resp = router
            .clone()
            .oneshot(req)
            .await
            .expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "update_backend should reject names containing '\\' with 400"
        );

        // Test with `..` in name — Axum normalizes `../` segments but not `..`
        // embedded within a segment. The validation catches this.
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends/foo..bar/update")
            .header(axum::http::header::COOKIE, cookie_header.as_str())
            .header("X-CSRF-Token", csrf_token)
            .body(Body::empty())
            .unwrap();

        let resp = router
            .clone()
            .oneshot(req)
            .await
            .expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "update_backend should reject names containing '..' with 400"
        );
    }
}

/// Query params for DELETE /tama/v1/backends/:name/versions/:version
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoveVersionQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// DELETE /tama/v1/backends/:name/versions/:version
pub async fn remove_backend_version(
    State(state): State<Arc<AppState>>,
    Path((name, version)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<RemoveVersionQuery>,
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
    let registry_result: Result<tama_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            tama_core::backends::BackendRegistry::open(&config_dir_clone)
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

    // Use gpu_variant from query param if provided
    let gpu_variant_filter = query.gpu_variant.clone();

    // Get the specific version record before deleting
    let versions = match registry.list_all_versions(&name, gpu_variant_filter.as_deref()) {
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

    // Find matching versions and check for ambiguity
    let matches: Vec<_> = versions.iter().filter(|v| v.version == version).collect();
    let info = match matches.len() {
        0 => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Backend '{}' version '{}' not found", name, version)
                })),
            )
                .into_response();
        }
        1 => matches[0].clone(),
        _ if gpu_variant_filter.is_some() => matches[0].clone(),
        _ => {
            // Multiple variants have the same version - require gpu_variant
            let variant_list: Vec<String> = matches.iter().map(|v| v.gpu_variant.clone()).collect();
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "Version '{}' exists in multiple variants for backend '{}'. Please specify gpu_variant. Available: {}",
                        version, name, variant_list.join(", ")
                    )
                })),
            )
                .into_response();
        }
    };

    // Delete files FIRST (before any DB changes)
    let info_to_remove = tama_core::backends::BackendInfo {
        name: info.name.clone(),
        backend_type: info.backend_type.clone(),
        version: info.version.clone(),
        path: std::path::PathBuf::from(&info.path),
        installed_at: info.installed_at,
        gpu_type: None,
        gpu_variant: info.gpu_variant.clone(),
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
        if let Err(e) = tama_core::backends::safe_remove_installation(&info_to_remove) {
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
    if let Err(e) = registry.remove_version(&name, &info.gpu_variant, &version) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to remove version from registry: {}", e)})),
        )
            .into_response();
    }

    // Clean up update_check record for this backend
    if let Ok(open) = tama_core::db::open(&config_dir) {
        let _ = tama_core::db::queries::delete_update_check(&open.conn, "backend", &name);
    }

    Json(DeleteResponse { removed: true }).into_response()
}

/// Query params for POST /tama/v1/backends/:name/activate
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivateQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// POST /tama/v1/backends/:name/activate
pub async fn activate_backend_version(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ActivateQuery>,
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

    // Determine gpu_variant: use explicit value or auto-infer from registry
    let gpu_variant = match query.gpu_variant {
        Some(v) => v,
        None => {
            let config_dir_clone = config_dir.clone();
            let name_clone = name.clone();
            let version_clone = req.version.clone();
            let infer_result: Result<Option<Vec<tama_core::backends::BackendInfo>>, anyhow::Error> =
                tokio::task::spawn_blocking(move || {
                    let reg = tama_core::backends::BackendRegistry::open(&config_dir_clone)?;
                    reg.list_all_versions(&name_clone, None)
                })
                .await
                .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
                .and_then(|r| r);

            let versions = match infer_result {
                Ok(Some(v)) => v,
                Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(serde_json::json!({
                            "error": format!("Backend '{}' not found", name)
                        })),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": format!("Failed to query backend: {}", e)
                        })),
                    )
                        .into_response();
                }
            };

            // Collect unique variants
            let mut variants: Vec<String> =
                versions.iter().map(|v| v.gpu_variant.clone()).collect();
            variants.sort();
            variants.dedup();

            if variants.len() == 1 {
                // Only one variant exists — use it
                variants.into_iter().next().unwrap()
            } else {
                // Multiple variants — find the one that has the requested version
                let matching: Vec<String> = versions
                    .iter()
                    .filter(|v| v.version == version_clone)
                    .map(|v| v.gpu_variant.clone())
                    .collect();
                let mut matching = matching;
                matching.sort();
                matching.dedup();

                match matching.len() {
                    1 => matching.into_iter().next().unwrap(),
                    0 => {
                        return (
                            StatusCode::NOT_FOUND,
                            Json(serde_json::json!({
                                "error": format!(
                                    "Version '{}' not found for backend '{}'. Available variants: {}",
                                    version_clone,
                                    name,
                                    variants.join(", ")
                                )
                            })),
                        )
                            .into_response();
                    }
                    _ => {
                        // Multiple variants have the same version — ambiguous
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({
                                "error": format!(
                                    "Version '{}' exists in multiple variants for backend '{}'. Please specify gpu_variant. Available variants: {}",
                                    version_clone,
                                    name,
                                    matching.join(", ")
                                )
                            })),
                        )
                            .into_response();
                    }
                }
            }
        }
    };

    let config_dir_clone = config_dir.clone();
    let version_clone = req.version.clone();
    let name_clone = name.clone();
    let version_for_error = version_clone.clone();
    let gpu_variant_clone = gpu_variant.to_string();
    let registry_result: Result<(tama_core::backends::BackendRegistry, bool), _> =
        tokio::task::spawn_blocking(move || {
            let mut reg = tama_core::backends::BackendRegistry::open(&config_dir_clone)?;
            let activated = reg.activate(&name_clone, &gpu_variant_clone, &version_clone)?;
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

/// POST /tama/v1/backends/:name/default-args
/// Update default_args for a backend in the backend_configs DB table.
#[derive(Deserialize)]
pub struct UpdateDefaultArgsRequest {
    pub default_args: Vec<String>,
}

/// Query params for POST /tama/v1/backends/:name/default-args
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DefaultArgsQuery {
    pub gpu_variant: String,
}

pub async fn update_backend_default_args(
    State(state): State<Arc<AppState>>,
    Path(backend_name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DefaultArgsQuery>,
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

    let backend_name = backend_name.clone();
    let gpu_variant = query.gpu_variant.clone();
    let default_args = req.default_args.clone();

    let result: Result<(), anyhow::Error> = tokio::task::spawn_blocking(move || {
        let open = tama_core::db::open(&config_dir)?;
        tama_core::db::queries::upsert_backend_config(
            &open.conn,
            &backend_name,
            &gpu_variant,
            &default_args,
            None,
        )?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
    .and_then(|r| r);

    match result {
        Ok(()) => Json(serde_json::json!({"success": true})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to update backend config: {}", e)})),
        )
            .into_response(),
    }
}
