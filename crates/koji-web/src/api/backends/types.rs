use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::time::Duration;

use crate::jobs::JobManager;
use koji_core::backends::ProgressSink;

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
    pub(super) fn default_uninstalled(
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

pub(super) fn job_to_active_dto(j: &crate::jobs::Job) -> ActiveJobDto {
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
// Job snapshot DTO
// ─────────────────────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────────────────────
// Known backends lookup
// ─────────────────────────────────────────────────────────────────────────────

pub(super) const KNOWN_BACKENDS: &[(&str, &str, Option<&str>)] = &[
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
    pub(super) jobs: Arc<JobManager>,
    pub(super) job: Arc<crate::jobs::Job>,
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
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create an ActiveJobDto for testing.
    fn make_active_dto(id: &str, kind: &str, backend_type: &str) -> ActiveJobDto {
        ActiveJobDto {
            id: id.to_string(),
            kind: kind.to_string(),
            backend_type: backend_type.to_string(),
        }
    }

    // ── ActiveJobDto serialization tests ──────────────────────────────────

    #[test]
    fn test_active_job_dto_serialization() {
        let dto = make_active_dto("job-123", "install", "llama_cpp");
        let json = serde_json::to_string(&dto).unwrap();
        let deserialized: ActiveJobDto = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "job-123");
        assert_eq!(deserialized.kind, "install");
        assert_eq!(deserialized.backend_type, "llama_cpp");
    }

    #[test]
    fn test_active_job_dto_update_kind() {
        let dto = make_active_dto("job-456", "update", "ik_llama");
        assert_eq!(dto.kind, "update");
        assert_eq!(dto.backend_type, "ik_llama");
    }

    #[test]
    fn test_active_job_dto_restore_kind() {
        let dto = make_active_dto("job-789", "restore", "custom");
        assert_eq!(dto.kind, "restore");
        assert_eq!(dto.backend_type, "custom");
    }

    #[test]
    fn test_active_job_dto_empty_backend() {
        let dto = make_active_dto("job-000", "install", "");
        assert_eq!(dto.backend_type, "");
    }

    // ── CapabilitiesDto serialization tests ───────────────────────────────

    #[test]
    fn test_capabilities_dto_serialization() {
        let caps = CapabilitiesDto {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            git_available: true,
            cmake_available: true,
            compiler_available: true,
            detected_cuda_version: Some("12.4".to_string()),
            supported_cuda_versions: vec!["12.0".to_string(), "12.4".to_string()],
        };

        let json = serde_json::to_string(&caps).unwrap();
        let deserialized: CapabilitiesDto = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.os, "linux");
        assert_eq!(deserialized.arch, "x86_64");
        assert!(deserialized.git_available);
        assert!(deserialized.cmake_available);
        assert!(deserialized.compiler_available);
        assert_eq!(deserialized.detected_cuda_version, Some("12.4".to_string()));
        assert_eq!(deserialized.supported_cuda_versions.len(), 2);
    }

    #[test]
    fn test_capabilities_dto_minimal() {
        let caps = CapabilitiesDto {
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            git_available: false,
            cmake_available: false,
            compiler_available: false,
            detected_cuda_version: None,
            supported_cuda_versions: vec![],
        };

        let json = serde_json::to_string(&caps).unwrap();
        let deserialized: CapabilitiesDto = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.os, "macos");
        assert!(!deserialized.git_available);
    }
}
