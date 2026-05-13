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

/// GET /tama/v1/backends
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
    let registry_result: Result<tama_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            tama_core::backends::BackendRegistry::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    // Load backend configs from DB (keyed by (name, gpu_variant))
    let backend_configs_map: std::collections::HashMap<(String, String), Vec<String>> =
        tama_core::backends::BackendManager::open(&config_dir)
            .ok()
            .map(|mgr| mgr.list_configs().ok())
            .flatten()
            .map(|configs| {
                configs
                    .into_iter()
                    .map(|c| ((c.name, c.gpu_variant), c.default_args))
                    .collect()
            })
            .unwrap_or_default();

    // Load cached update checks from DB (keyed by "name:variant")
    let update_checks: std::collections::HashMap<
        String,
        tama_core::db::queries::UpdateCheckRecord,
    > = tama_core::db::open(&config_dir)
        .ok()
        .and_then(|open| tama_core::db::queries::get_all_update_checks(&open.conn).ok())
        .map(|records| {
            records
                .into_iter()
                .filter(|r| r.item_type == "backend")
                .map(|r| (r.item_id.clone(), r))
                .collect()
        })
        .unwrap_or_default();

    // Build the response including available backend types
    let mut backends: Vec<BackendCardDto> = Vec::new();
    let mut custom: Vec<BackendCardDto> = Vec::new();
    let mut available: Vec<String> = Vec::new();

    match registry_result {
        Ok(registry) => {
            // Emit one card per (backend_type, gpu_variant) pair — only if installed
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                let versions_opt = registry.list_all_versions(type_, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    // Create one card per variant
                    for (variant, variant_versions) in variant_groups {
                        let default_args = backend_configs_map
                            .get(&(type_.to_string(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let active_version = registry.get(type_, &variant).ok().flatten();

                        // Sort versions by installed_at DESC
                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        // Build version DTOs
                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
                                gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
                                source: info.source.as_ref().map(|s| s.into()),
                                is_active: active_version
                                    .as_ref()
                                    .map(|a| a.version == info.version)
                                    .unwrap_or(false),
                            })
                            .collect();

                        let active_info = active_version.map(BackendInfoDto::from);

                        // Load cached update status from DB (keyed by "name:variant")
                        let update_key = format!("{}:{}", type_, variant);
                        let update_status = update_checks
                            .get(&update_key)
                            .map(|r| UpdateStatusDto {
                                checked: true,
                                latest_version: r.latest_version.clone(),
                                update_available: if r.update_available {
                                    Some(true)
                                } else {
                                    None
                                },
                            })
                            .unwrap_or_default();

                        backends.push(BackendCardDto {
                            r#type: type_.to_string(),
                            display_name: display_name.to_string(),
                            installed: true,
                            gpu_variant: variant,
                            info: active_info,
                            versions: version_dtos,
                            update: update_status,
                            release_notes_url: release_notes_url.map(String::from),
                            default_args: default_args.clone(),
                            is_active: true,
                        });
                    }
                } else {
                    available.push(type_.to_string());
                }
            }

            // Custom backends — one card per (name, variant) pair
            // Collect unique custom backend names to avoid duplicate cards
            // when multiple variants are active for the same backend
            let active_backends = registry.list().unwrap_or_default();
            let mut custom_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for active in &active_backends {
                let bt = active.backend_type.to_string();
                if !matches!(bt.as_str(), "llama_cpp" | "ik_llama" | "tts_kokoro") {
                    custom_names.insert(active.name.clone());
                }
            }

            for name in &custom_names {
                let versions_opt = registry.list_all_versions(name, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let bt = versions
                        .first()
                        .map(|v| v.backend_type.to_string())
                        .unwrap_or_default();

                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    for (variant, variant_versions) in variant_groups {
                        let active_version = registry.get(name, &variant).ok().flatten();
                        let default_args = backend_configs_map
                            .get(&(bt.clone(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
                                gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
                                source: info.source.as_ref().map(|s| s.into()),
                                is_active: active_version
                                    .as_ref()
                                    .map(|a| a.version == info.version)
                                    .unwrap_or(false),
                            })
                            .collect();

                        let active_info = active_version.map(BackendInfoDto::from);

                        // Load cached update status from DB (keyed by "name:variant")
                        let update_key = format!("{}:{}", name, variant);
                        let update_status = update_checks
                            .get(&update_key)
                            .map(|r| UpdateStatusDto {
                                checked: true,
                                latest_version: r.latest_version.clone(),
                                update_available: if r.update_available {
                                    Some(true)
                                } else {
                                    None
                                },
                            })
                            .unwrap_or_default();

                        custom.push(BackendCardDto {
                            r#type: bt.clone(),
                            display_name: format!("Custom ({})", name),
                            installed: true,
                            gpu_variant: variant,
                            info: active_info,
                            versions: version_dtos,
                            update: update_status,
                            release_notes_url: None,
                            default_args,
                            is_active: true,
                        });
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to open backend registry: {}", e);
        }
    }

    Json(BackendListResponse {
        active_job,
        backends,
        custom,
        available,
    })
    .into_response()
}

/// POST /tama/v1/backends/check-updates
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
    let registry_result: Result<tama_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            tama_core::backends::BackendRegistry::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    // Load backend configs from DB (keyed by (name, gpu_variant))
    let backend_configs_map: std::collections::HashMap<(String, String), Vec<String>> =
        tama_core::backends::BackendManager::open(&config_dir)
            .ok()
            .map(|mgr| mgr.list_configs().ok())
            .flatten()
            .map(|configs| {
                configs
                    .into_iter()
                    .map(|c| ((c.name, c.gpu_variant), c.default_args))
                    .collect()
            })
            .unwrap_or_default();

    let mut backends: Vec<BackendCardDto> = Vec::new();
    let mut custom: Vec<BackendCardDto> = Vec::new();

    match registry_result {
        Ok(registry) => {
            // Emit one card per (backend_type, gpu_variant) pair
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                let versions_opt = registry.list_all_versions(type_, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    // Create one card per variant
                    for (variant, variant_versions) in variant_groups {
                        let default_args = backend_configs_map
                            .get(&(type_.to_string(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let active_version = registry.get(type_, &variant).ok().flatten();

                        // Check for updates against the active version
                        let update_check = match active_version.as_ref() {
                            Some(info) => match tama_core::backends::check_updates(info).await {
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
                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
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
                            gpu_variant: variant,
                            info: active_info,
                            versions: version_dtos,
                            update: UpdateStatusDto {
                                checked: update_check.checked,
                                latest_version: update_check.latest_version.clone(),
                                update_available: update_check.update_available,
                            },
                            release_notes_url: release_notes_url.map(String::from),
                            default_args: default_args.clone(),
                            is_active: true,
                        });
                    }
                } else {
                    backends.push(BackendCardDto::default_uninstalled(
                        type_,
                        display_name,
                        *release_notes_url,
                        Vec::new(),
                    ));
                }
            }

            // Custom backends — one card per (name, variant) pair
            // Collect unique custom backend names to avoid duplicate cards
            let active_backends = registry.list().unwrap_or_default();
            let mut custom_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for active in &active_backends {
                let bt = active.backend_type.to_string();
                if !matches!(bt.as_str(), "llama_cpp" | "ik_llama" | "tts_kokoro") {
                    custom_names.insert(active.name.clone());
                }
            }

            for name in &custom_names {
                let versions_opt = registry.list_all_versions(name, None).unwrap_or(None);

                if let Some(versions) = versions_opt {
                    let bt = versions
                        .first()
                        .map(|v| v.backend_type.to_string())
                        .unwrap_or_default();

                    // Group versions by gpu_variant
                    let mut variant_groups: std::collections::HashMap<String, Vec<_>> =
                        std::collections::HashMap::new();
                    for info in &versions {
                        variant_groups
                            .entry(info.gpu_variant.clone())
                            .or_default()
                            .push(info.clone());
                    }

                    for (variant, variant_versions) in variant_groups {
                        let active_version = registry.get(name, &variant).ok().flatten();
                        let default_args = backend_configs_map
                            .get(&(bt.clone(), variant.clone()))
                            .cloned()
                            .unwrap_or_default();

                        let mut sorted_versions = variant_versions;
                        sorted_versions.sort_by_key(|b| std::cmp::Reverse(b.installed_at));

                        let version_dtos: Vec<BackendVersionDto> = sorted_versions
                            .iter()
                            .map(|info| BackendVersionDto {
                                name: info.name.clone(),
                                version: info.version.clone(),
                                path: info.path.to_string_lossy().to_string(),
                                installed_at: info.installed_at,
                                gpu_variant: info.gpu_variant.clone(),
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
                            r#type: bt.clone(),
                            display_name: format!("Custom ({})", name),
                            installed: true,
                            gpu_variant: variant,
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
        }
        Err(e) => {
            tracing::warn!("Failed to open backend registry: {}", e);
            // On error, still return known backends as not installed
            for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
                backends.push(BackendCardDto::default_uninstalled(
                    type_,
                    display_name,
                    *release_notes_url,
                    Vec::new(),
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

/// GET /tama/v1/backends/:name/versions
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
    let registry_result: Result<tama_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            tama_core::backends::BackendRegistry::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    match registry_result {
        Ok(registry) => {
            let versions_opt = match registry.list_all_versions(&name, None) {
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

            // Get the active version for this backend, keyed by (name, gpu_variant)
            let active_backends: Vec<_> = registry
                .list()
                .ok()
                .map(|backends| {
                    backends
                        .into_iter()
                        .filter(|b| b.name == name)
                        .map(|b| (b.gpu_variant, b.version))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let dto_versions: Vec<BackendVersionDto> = versions
                .iter()
                .map(|info| {
                    let is_active = active_backends.iter().any(|(variant, version)| {
                        variant == &info.gpu_variant && version == &info.version
                    });
                    BackendVersionDto {
                        name: info.name.clone(),
                        version: info.version.clone(),
                        path: info.path.to_string_lossy().to_string(),
                        installed_at: info.installed_at,
                        gpu_variant: info.gpu_variant.clone(),
                        gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
                        source: info.source.as_ref().map(|s| s.into()),
                        is_active,
                    }
                })
                .collect();

            let active_version = active_backends.first().map(|(_, v)| v.clone());

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
