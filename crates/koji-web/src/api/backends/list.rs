use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;
use std::sync::Arc;

use super::types::*;
use crate::server::AppState;

/// GET /api/backends
pub async fn list_backends(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // active_job is only available when job manager is configured
    let active_job = if let Some(jobs) = &state.jobs {
        jobs.active()
            .await
            .filter(|j| {
                let st = j.state.try_read().ok();
                if let Some(s) = &st {
                    matches!(s.status, crate::jobs::JobStatus::Running)
                } else {
                    false
                }
            })
            .map(|j| job_to_active_dto(&j))
    } else {
        None
    };

    // Open registry
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

    // Open registry (blocking call wrapped in spawn_blocking)
    let config_dir_clone = config_dir.clone();
    let registry_result: Result<koji_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            koji_core::backends::BackendRegistry::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    // Load config to get default_args
    let config_result = koji_core::config::Config::load_from(&config_dir);
    let default_args_map: std::collections::HashMap<String, Vec<String>> = config_result
        .ok()
        .map(|cfg| {
            cfg.backends
                .iter()
                .map(|(k, v)| (k.clone(), v.default_args.clone()))
                .collect()
        })
        .unwrap_or_default();

    let mut backends: Vec<BackendCardDto> = Vec::new();
    let mut custom: Vec<BackendCardDto> = Vec::new();

    match registry_result {
        Ok(registry) => {
            // Emit one card per backend type with all versions in a `versions` array
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                let default_args = default_args_map
                    .get(&type_.to_string())
                    .cloned()
                    .unwrap_or_default();

                // Get ALL versions for this backend type
                let versions_opt = registry.list_all_versions(type_).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let active_version = registry.get(type_).ok().flatten();

                    // Sort versions by installed_at DESC
                    let mut sorted_versions = versions.clone();
                    sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                    // Build version DTOs
                    let version_dtos: Vec<BackendVersionDto> = sorted_versions
                        .iter()
                        .map(|info| BackendVersionDto {
                            name: info.name.clone(),
                            version: info.version.clone(),
                            path: info.path.to_string_lossy().to_string(),
                            installed_at: info.installed_at,
                            gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
                            source: info.source.as_ref().map(|s| s.into()),
                            is_active: active_version
                                .as_ref()
                                .map(|a| a.version == info.version)
                                .unwrap_or(false),
                        })
                        .collect();

                    let active_info = active_version.map(BackendInfoDto::from);

                    backends.push(BackendCardDto {
                        r#type: type_.to_string(),
                        display_name: display_name.to_string(),
                        installed: true,
                        info: active_info,
                        versions: version_dtos,
                        update: UpdateStatusDto::default(),
                        release_notes_url: release_notes_url.map(String::from),
                        default_args,
                        is_active: true,
                    });
                } else {
                    // No versions installed — show uninstalled card
                    backends.push(BackendCardDto::default_uninstalled(
                        type_,
                        display_name,
                        *release_notes_url,
                        default_args,
                    ));
                }
            }

            // Custom backends — one card per backend with all versions
            let active_backends = registry.list().unwrap_or_default();

            for active in &active_backends {
                let bt = active.backend_type.to_string();
                if bt == "llama_cpp" || bt == "ik_llama" {
                    continue;
                }

                let versions_opt = registry.list_all_versions(&active.name).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let active_version = registry.get(&active.name).ok().flatten();
                    let default_args = default_args_map.get(&bt).cloned().unwrap_or_default();

                    let mut sorted_versions = versions.clone();
                    sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                    let version_dtos: Vec<BackendVersionDto> = sorted_versions
                        .iter()
                        .map(|info| BackendVersionDto {
                            name: info.name.clone(),
                            version: info.version.clone(),
                            path: info.path.to_string_lossy().to_string(),
                            installed_at: info.installed_at,
                            gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
                            source: info.source.as_ref().map(|s| s.into()),
                            is_active: active_version
                                .as_ref()
                                .map(|a| a.version == info.version)
                                .unwrap_or(false),
                        })
                        .collect();

                    let active_info = active_version.map(BackendInfoDto::from);

                    custom.push(BackendCardDto {
                        r#type: format!("{}", active.backend_type),
                        display_name: format!("Custom ({})", active.name),
                        installed: true,
                        info: active_info,
                        versions: version_dtos,
                        update: UpdateStatusDto::default(),
                        release_notes_url: None,
                        default_args,
                        is_active: true,
                    });
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to open backend registry: {}", e);
            // On error, still return known backends as not installed
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                let default_args = default_args_map
                    .get(&type_.to_string())
                    .cloned()
                    .unwrap_or_default();
                backends.push(BackendCardDto::default_uninstalled(
                    type_,
                    display_name,
                    *release_notes_url,
                    default_args,
                ));
            }
        }
    }

    Json(BackendListResponse {
        active_job,
        backends,
        custom,
    })
    .into_response()
}

/// POST /api/backends/check-updates
pub async fn check_backend_updates(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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

    // Get active job if any
    let active_job = jobs
        .active()
        .await
        .filter(|j| {
            let state = j.state.try_read().ok();
            if let Some(s) = &state {
                matches!(s.status, crate::jobs::JobStatus::Running)
            } else {
                false
            }
        })
        .map(|j| job_to_active_dto(&j));

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

    // Open registry
    let config_dir_clone = config_dir.clone();
    let registry_result: Result<koji_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            koji_core::backends::BackendRegistry::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    // Load config to get default_args
    let config_result = koji_core::config::Config::load_from(&config_dir);
    let default_args_map: std::collections::HashMap<String, Vec<String>> = config_result
        .ok()
        .map(|cfg| {
            cfg.backends
                .iter()
                .map(|(k, v)| (k.clone(), v.default_args.clone()))
                .collect()
        })
        .unwrap_or_default();

    let mut backends: Vec<BackendCardDto> = Vec::new();
    let mut custom: Vec<BackendCardDto> = Vec::new();

    match registry_result {
        Ok(registry) => {
            // Emit one card per backend type with all versions in a `versions` array
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                let default_args = default_args_map
                    .get(&type_.to_string())
                    .cloned()
                    .unwrap_or_default();

                let versions_opt = registry.list_all_versions(type_).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let active_version = registry.get(type_).ok().flatten();

                    // Check for updates against the active version
                    let update_check = match active_version.as_ref() {
                        Some(info) => match koji_core::backends::check_updates(info).await {
                            Ok(check) => UpdateStatusDto {
                                checked: true,
                                latest_version: Some(check.latest_version),
                                update_available: Some(check.update_available),
                            },
                            Err(_) => UpdateStatusDto {
                                checked: true,
                                latest_version: None,
                                update_available: None,
                            },
                        },
                        None => UpdateStatusDto::default(),
                    };

                    // Sort versions by installed_at DESC
                    let mut sorted_versions = versions.clone();
                    sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                    let version_dtos: Vec<BackendVersionDto> = sorted_versions
                        .iter()
                        .map(|info| BackendVersionDto {
                            name: info.name.clone(),
                            version: info.version.clone(),
                            path: info.path.to_string_lossy().to_string(),
                            installed_at: info.installed_at,
                            gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
                            source: info.source.as_ref().map(|s| s.into()),
                            is_active: active_version
                                .as_ref()
                                .map(|a| a.version == info.version)
                                .unwrap_or(false),
                        })
                        .collect();

                    let active_info = active_version.map(BackendInfoDto::from);

                    backends.push(BackendCardDto {
                        r#type: type_.to_string(),
                        display_name: display_name.to_string(),
                        installed: true,
                        info: active_info,
                        versions: version_dtos,
                        update: UpdateStatusDto {
                            checked: update_check.checked,
                            latest_version: update_check.latest_version.clone(),
                            update_available: update_check.update_available,
                        },
                        release_notes_url: release_notes_url.map(String::from),
                        default_args,
                        is_active: true,
                    });
                } else {
                    backends.push(BackendCardDto::default_uninstalled(
                        type_,
                        display_name,
                        *release_notes_url,
                        default_args,
                    ));
                }
            }

            // Custom backends — one card per backend with all versions
            let active_backends = registry.list().unwrap_or_default();
            for active in &active_backends {
                let bt = active.backend_type.to_string();
                if bt == "llama_cpp" || bt == "ik_llama" {
                    continue;
                }

                let versions_opt = registry.list_all_versions(&active.name).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let active_version = registry.get(&active.name).ok().flatten();
                    let default_args = default_args_map.get(&bt).cloned().unwrap_or_default();

                    let mut sorted_versions = versions.clone();
                    sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                    let version_dtos: Vec<BackendVersionDto> = sorted_versions
                        .iter()
                        .map(|info| BackendVersionDto {
                            name: info.name.clone(),
                            version: info.version.clone(),
                            path: info.path.to_string_lossy().to_string(),
                            installed_at: info.installed_at,
                            gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
                            source: info.source.as_ref().map(|s| s.into()),
                            is_active: active_version
                                .as_ref()
                                .map(|a| a.version == info.version)
                                .unwrap_or(false),
                        })
                        .collect();

                    let active_info = active_version.map(BackendInfoDto::from);

                    custom.push(BackendCardDto {
                        r#type: format!("{}", active.backend_type),
                        display_name: format!("Custom ({})", active.name),
                        installed: true,
                        info: active_info,
                        versions: version_dtos,
                        update: UpdateStatusDto::default(),
                        release_notes_url: None,
                        default_args,
                        is_active: true,
                    });
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to open backend registry: {}", e);
            // On error, still return known backends as not installed
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                let default_args = default_args_map
                    .get(&type_.to_string())
                    .cloned()
                    .unwrap_or_default();
                backends.push(BackendCardDto::default_uninstalled(
                    type_,
                    display_name,
                    *release_notes_url,
                    default_args,
                ));
            }
        }
    }

    Json(CheckUpdatesResponse {
        active_job,
        backends,
        custom,
    })
    .into_response()
}

/// GET /api/backends/:name/versions
pub async fn list_backend_versions(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    // Validate name (prevent path traversal)
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid backend name"})),
        )
            .into_response();
    }

    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "config_path not configured"})),
            )
                .into_response();
        }
    };

    let config_dir = match config_path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Cannot determine config directory"})),
            )
                .into_response();
        }
    };

    let config_dir_clone = config_dir.clone();
    let registry_result: Result<koji_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            koji_core::backends::BackendRegistry::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    match registry_result {
        Ok(registry) => {
            let versions_opt = match registry.list_all_versions(&name) {
                Ok(v) => v,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("Failed to list versions: {}", e)})),
                    )
                        .into_response();
                }
            };

            let versions = match versions_opt {
                Some(v) => v,
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(json!({"error": format!("Backend '{}' not found", name)})),
                    )
                        .into_response();
                }
            };

            // Get the active version for comparison
            let active_version = registry.get(&name).ok().flatten().map(|a| a.version);

            let dto_versions: Vec<BackendVersionDto> = versions
                .iter()
                .map(|info| BackendVersionDto {
                    name: info.name.clone(),
                    version: info.version.clone(),
                    path: info.path.to_string_lossy().to_string(),
                    installed_at: info.installed_at,
                    gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
                    source: info.source.as_ref().map(|s| s.into()),
                    is_active: active_version.as_deref() == Some(&info.version),
                })
                .collect();

            Json(BackendVersionsResponse {
                versions: dto_versions,
                active_version,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Failed to open registry: {}", e)})),
        )
            .into_response(),
    }
}
