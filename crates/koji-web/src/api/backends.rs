#![allow(clippy::unnecessary_to_owned)]

use async_stream::stream;
use axum::response::sse::{Event, KeepAlive};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Sse},
    Json,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::time::Duration;

use crate::jobs::JobManager;
use crate::server::AppState;
use koji_core::backends::{BackendInfo, ProgressSink};

// ─────────────────────────────────────────────────────────────────────────────
// Wire DTOs (koji-web only, not exposed from koji-core)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendListResponse {
    pub active_job: Option<ActiveJobDto>,
    pub backends: Vec<BackendCardDto>,
    pub custom: Vec<BackendCardDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendCardDto {
    pub r#type: String,
    pub display_name: String,
    pub installed: bool,
    /// Info for the currently active version (shown by default in the UI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<BackendInfoDto>,
    /// All installed versions of this backend.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<BackendVersionDto>,
    #[serde(skip_serializing_if = "UpdateStatusDto::is_default")]
    pub update: UpdateStatusDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes_url: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    /// Whether the active version is currently selected for display.
    #[serde(default)]
    pub is_active: bool,
}

impl BackendCardDto {
    fn default_uninstalled(
        type_: &str,
        display_name: &str,
        release_notes_url: Option<&str>,
        default_args: Vec<String>,
    ) -> Self {
        Self {
            r#type: type_.to_string(),
            display_name: display_name.to_string(),
            installed: false,
            info: None,
            versions: vec![],
            update: UpdateStatusDto::default(),
            release_notes_url: release_notes_url.map(String::from),
            default_args,
            is_active: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendInfoDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_type: Option<GpuTypeDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSourceDto>,
}

impl From<koji_core::backends::BackendInfo> for BackendInfoDto {
    fn from(info: koji_core::backends::BackendInfo) -> Self {
        Self {
            name: info.name,
            version: info.version,
            path: info.path.to_string_lossy().to_string(),
            installed_at: info.installed_at,
            gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
            source: info.source.as_ref().map(|s| s.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GpuTypeDto {
    Cuda { version: String },
    Vulkan,
    Metal,
    Rocm { version: String },
    CpuOnly,
    Custom,
}

impl From<&koji_core::gpu::GpuType> for GpuTypeDto {
    fn from(gpu: &koji_core::gpu::GpuType) -> Self {
        match gpu {
            koji_core::gpu::GpuType::Cuda { version } => Self::Cuda {
                version: version.clone(),
            },
            koji_core::gpu::GpuType::Vulkan => Self::Vulkan,
            koji_core::gpu::GpuType::Metal => Self::Metal,
            koji_core::gpu::GpuType::RocM { version } => Self::Rocm {
                version: version.clone(),
            },
            koji_core::gpu::GpuType::CpuOnly => Self::CpuOnly,
            koji_core::gpu::GpuType::Custom => Self::Custom,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BackendSourceDto {
    Prebuilt {
        version: String,
    },
    SourceCode {
        version: String,
        git_url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
}

impl From<&koji_core::backends::BackendSource> for BackendSourceDto {
    fn from(source: &koji_core::backends::BackendSource) -> Self {
        match source {
            koji_core::backends::BackendSource::Prebuilt { version } => Self::Prebuilt {
                version: version.clone(),
            },
            koji_core::backends::BackendSource::SourceCode {
                version,
                git_url,
                commit,
            } => Self::SourceCode {
                version: version.clone(),
                git_url: git_url.clone(),
                commit: commit.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct UpdateStatusDto {
    pub checked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_available: Option<bool>,
}

impl UpdateStatusDto {
    pub fn is_default(&self) -> bool {
        !self.checked
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActiveJobDto {
    pub id: String,
    pub kind: String,
    pub backend_type: String,
}

fn job_to_active_dto(j: &crate::jobs::Job) -> ActiveJobDto {
    ActiveJobDto {
        id: j.id.clone(),
        kind: match j.kind {
            crate::jobs::JobKind::Install => "install".to_string(),
            crate::jobs::JobKind::Update => "update".to_string(),
            crate::jobs::JobKind::Restore => "restore".to_string(),
        },
        backend_type: match j.backend_type.as_ref() {
            Some(koji_core::backends::BackendType::LlamaCpp) => "llama_cpp".to_string(),
            Some(koji_core::backends::BackendType::IkLlama) => "ik_llama".to_string(),
            Some(koji_core::backends::BackendType::Custom) => "custom".to_string(),
            None => String::new(),
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CapabilitiesDto {
    pub os: String,
    pub arch: String,
    pub git_available: bool,
    pub cmake_available: bool,
    pub compiler_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_cuda_version: Option<String>,
    pub supported_cuda_versions: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Request/Response DTOs for mutation API
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallRequest {
    pub backend_type: String,
    pub version: Option<String>,
    pub gpu_type: GpuTypeDto,
    pub build_from_source: bool,
    pub force: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallResponse {
    pub job_id: String,
    pub kind: String,
    pub backend_type: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DeleteResponse {
    pub removed: bool,
}

/// Version info returned by the versions endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendVersionDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_type: Option<GpuTypeDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSourceDto>,
    pub is_active: bool,
}

/// Response for GET /api/backends/:name/versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendVersionsResponse {
    pub versions: Vec<BackendVersionDto>,
    pub active_version: Option<String>,
}

/// Request body for POST /api/backends/:name/activate.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivateRequest {
    pub version: String,
}

/// Response for POST /api/backends/:name/activate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivateResponse {
    pub version: String,
    pub is_active: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CheckUpdatesResponse {
    pub active_job: Option<ActiveJobDto>,
    pub backends: Vec<BackendCardDto>,
    pub custom: Vec<BackendCardDto>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Known backends lookup
// ─────────────────────────────────────────────────────────────────────────────

const KNOWN_BACKENDS: &[(&str, &str, Option<&str>)] = &[
    (
        "llama_cpp",
        "llama.cpp",
        Some("https://github.com/ggml-org/llama.cpp/releases"),
    ),
    (
        "ik_llama",
        "ik_llama.cpp",
        Some("https://github.com/ikawrakow/ik_llama.cpp/commits/main"),
    ),
];

// ─────────────────────────────────────────────────────────────────────────────
// Job adapter for progress streaming
// ─────────────────────────────────────────────────────────────────────────────

pub struct JobAdapter {
    jobs: Arc<JobManager>,
    job: Arc<crate::jobs::Job>,
}

impl ProgressSink for JobAdapter {
    fn log(&self, line: &str) {
        let jobs = self.jobs.clone();
        let job = self.job.clone();
        let line = line.to_string();
        // ProgressSink::log is sync; we need to call async append_log.
        // Use tokio::runtime::Handle::current().spawn — installer runs inside the runtime.
        tokio::runtime::Handle::current().spawn(async move {
            jobs.append_log(&job, line).await;
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Capabilities cache
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct CapabilitiesCache {
    inner: Arc<tokio::sync::Mutex<Option<(std::time::Instant, CapabilitiesDto)>>>,
}

impl CapabilitiesCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub async fn get_or_compute(
        &self,
        detect_prereqs: fn() -> koji_core::gpu::BuildPrerequisites,
        detect_cuda: fn() -> Option<String>,
    ) -> anyhow::Result<CapabilitiesDto> {
        let now = std::time::Instant::now();
        let mut guard = self.inner.lock().await;

        // Check cache hit (5-second TTL)
        if let Some((cached_at, cached)) = &*guard {
            if now.duration_since(*cached_at) < Duration::from_secs(5) {
                return Ok(cached.clone());
            }
        }

        // Cold path: spawn_blocking to avoid blocking runtime
        let result = tokio::task::spawn_blocking(move || {
            let caps = detect_prereqs();
            let cuda = detect_cuda();
            CapabilitiesDto {
                os: caps.os,
                arch: caps.arch,
                git_available: caps.git_available,
                cmake_available: caps.cmake_available,
                compiler_available: caps.compiler_available,
                detected_cuda_version: cuda,
                supported_cuda_versions: vec![
                    "11.1".to_string(),
                    "12.4".to_string(),
                    "13.1".to_string(),
                ],
            }
        })
        .await;

        let caps = match result {
            Ok(c) => c,
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to detect capabilities: {}", e));
            }
        };

        *guard = Some((now, caps.clone()));
        Ok(caps)
    }
}

impl Default for CapabilitiesCache {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET /api/system/capabilities
pub async fn system_capabilities(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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

    match cache
        .get_or_compute(
            koji_core::gpu::detect_build_prerequisites,
            koji_core::gpu::detect_cuda_version,
        )
        .await
    {
        Ok(caps) => Json(caps).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

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

/// POST /api/backends/install
pub async fn install_backend(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InstallRequest>,
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
                git_url: match backend_type {
                    koji_core::backends::BackendType::LlamaCpp => {
                        "https://github.com/ggml-org/llama.cpp.git"
                    }
                    koji_core::backends::BackendType::IkLlama => {
                        "https://github.com/ikawrakow/ik_llama.cpp.git"
                    }
                    _ => "https://github.com/ggml-org/llama.cpp.git",
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

/// DELETE /api/backends/:name
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
    let info_to_remove = BackendInfo {
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
                    Json(json!({
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
            Json(json!({"error": format!("Failed to activate: {}", e)})),
        )
            .into_response(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Job snapshot and SSE handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Job snapshot DTO
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub struct JobSnapshotDto {
    pub id: String,
    pub kind: String,
    pub status: crate::jobs::JobStatus,
    pub backend_type: String,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<String>,
}

/// GET /api/backends/jobs/:id
#[allow(dead_code)]
pub async fn get_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<JobSnapshotDto>, StatusCode> {
    let jobs = state
        .jobs
        .as_ref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let job = jobs.get(&job_id).await.ok_or(StatusCode::NOT_FOUND)?;

    let (state, log_head, log_tail, dropped) = tokio::join!(
        job.state.read(),
        job.log_head.read(),
        job.log_tail.read(),
        async { job.log_dropped.load(Ordering::Relaxed) }
    );

    let mut log: Vec<String> = log_head.iter().cloned().collect();
    if dropped > 0 && !log_tail.is_empty() {
        log.push(format!("[... {} lines skipped ...]", dropped));
    }
    log.extend(log_tail.iter().cloned());

    Ok(Json(JobSnapshotDto {
        id: job.id.clone(),
        kind: match job.kind {
            crate::jobs::JobKind::Install => "install".to_string(),
            crate::jobs::JobKind::Update => "update".to_string(),
            crate::jobs::JobKind::Restore => "restore".to_string(),
        },
        status: state.status,
        backend_type: job
            .backend_type
            .as_ref()
            .map(|b| b.to_string())
            .unwrap_or_default(),
        started_at: state.started_at,
        finished_at: state.finished_at,
        error: state.error.clone(),
        log,
    }))
}

/// GET /api/backends/jobs/:id/events
#[allow(dead_code)]
pub async fn job_events_sse(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
    let jobs = state
        .jobs
        .as_ref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let job = jobs.get(&job_id).await.ok_or(StatusCode::NOT_FOUND)?;

    let mut rx = job.log_tx.subscribe();

    // Snapshot + subscribe: take both under the same lock to avoid race
    let (head, tail, dropped, status, _finished_at, error) = {
        let (state, log_head, log_tail) =
            tokio::join!(job.state.read(), job.log_head.read(), job.log_tail.read());
        (
            log_head.iter().cloned().collect::<Vec<_>>(),
            log_tail.iter().cloned().collect::<Vec<_>>(),
            job.log_dropped.load(Ordering::Relaxed),
            state.status,
            state.finished_at,
            state.error.clone(),
        )
    };

    let stream = stream! {
        // Replay head
        for line in head {
            yield Ok(Event::default().event("log").json_data(json!({ "line": line}))?);
        }

        // Emit skipped marker if dropped > 0
        if dropped > 0 && !tail.is_empty() {
            yield Ok(Event::default().event("log")
                .json_data(json!({ "line": format!("[... {} lines skipped ...]", dropped)}))?);
        }

        // Replay tail
        for line in tail {
            yield Ok(Event::default().event("log").json_data(json!({ "line": line}))?);
        }

        // Emit final status if terminal
        if status != crate::jobs::JobStatus::Running {
            yield Ok(Event::default().event("status")
                .json_data(json!({ "status": status}))?);
            if let Some(err) = error {
                yield Ok(Event::default().event("error")
                    .json_data(json!({ "error": err}))?);
            }
            return; // Close after terminal job
        }

        // Live stream
        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Ok(crate::jobs::JobEvent::Log(line)) => {
                            yield Ok(Event::default().event("log")
                                .json_data(json!({ "line": line}))?);
                        }
                        Ok(crate::jobs::JobEvent::Status(s)) => {
                            yield Ok(Event::default().event("status")
                                .json_data(json!({ "status": s}))?);
                            if s != crate::jobs::JobStatus::Running {
                                return; // Close on terminal status
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            // Emit dropped marker
                            yield Ok(Event::default().event("log")
                                .json_data(json!({ "line": format!("[{} lines dropped]", n)}))?);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return;
                        }
                    }
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
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
