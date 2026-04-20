use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::queries::{
    activate_backend_version, delete_all_backend_versions, delete_backend_installation,
    get_active_backend, get_backend_by_version, insert_backend_installation, list_active_backends,
    list_backend_versions, BackendInstallationRecord,
};
use crate::gpu::GpuType;

/// Metadata for an installed backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub name: String,
    pub backend_type: BackendType,
    pub version: String,
    pub path: PathBuf,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_type: Option<GpuType>,
    #[serde(default)]
    pub source: Option<BackendSource>,
}

/// Source of a backend installation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", content = "content")]
pub enum BackendSource {
    Prebuilt {
        version: String,
    },
    SourceCode {
        version: String,
        git_url: String,
        /// Optional specific commit hash to check out after cloning.
        /// When set, the clone uses enough depth to reach the commit and
        /// then runs `git checkout <commit>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
}

pub struct BackendRegistry {
    conn: Connection,
    /// Shared HTTP client for all backend downloads.
    /// Using a single client enables connection pooling and is more
    /// testable than creating clients per-request or using lazy_static.
    pub client: reqwest::Client,
}

impl BackendRegistry {
    /// Open a BackendRegistry backed by SQLite at `<config_dir>/koji.db`.
    pub fn open(config_dir: &Path) -> Result<Self> {
        let open_result = crate::db::open(config_dir)?;
        Ok(Self {
            conn: open_result.conn,
            client: Self::make_client(),
        })
    }

    /// Open an in-memory BackendRegistry for testing.
    pub fn open_in_memory() -> Result<Self> {
        let open_result = crate::db::open_in_memory()?;
        Ok(Self {
            conn: open_result.conn,
            client: Self::make_client(),
        })
    }

    /// Create a shared reqwest::Client configured for backend downloads.
    fn make_client() -> reqwest::Client {
        reqwest::Client::builder()
            .user_agent("koji-backend-manager")
            .timeout(std::time::Duration::from_secs(300))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client for backend downloads")
    }

    /// Add a new backend installation, marking it as the active version.
    pub fn add(&mut self, backend: BackendInfo) -> Result<()> {
        let record = Self::backend_info_to_record(&backend)?;
        insert_backend_installation(&self.conn, &record)
            .with_context(|| format!("Failed to insert backend '{}'", backend.name))
    }

    /// Remove all versions of a backend by name.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        delete_all_backend_versions(&self.conn, name)
            .with_context(|| format!("Failed to remove backend '{}'", name))
    }

    /// Get the active backend installation for a given name.
    ///
    /// Returns `Ok(None)` if no backend with that name exists.
    pub fn get(&self, name: &str) -> Result<Option<BackendInfo>> {
        let record = get_active_backend(&self.conn, name)
            .with_context(|| format!("Failed to query backend '{}'", name))?;
        match record {
            Some(r) => Ok(Some(Self::record_to_backend_info(r)?)),
            None => Ok(None),
        }
    }

    /// List all active backend installations.
    pub fn list(&self) -> Result<Vec<BackendInfo>> {
        let records =
            list_active_backends(&self.conn).with_context(|| "Failed to list active backends")?;
        records
            .into_iter()
            .map(Self::record_to_backend_info)
            .collect()
    }

    /// Update an installed backend to a new version.
    ///
    /// Constructs a new `BackendInfo` with updated fields and calls `add()`,
    /// which marks the new row active and deactivates the old one.
    pub fn update_version(
        &mut self,
        name: &str,
        new_version: String,
        new_binary_path: PathBuf,
        new_source: Option<BackendSource>,
    ) -> Result<()> {
        let existing = self
            .get(name)?
            .ok_or_else(|| anyhow!("Backend '{}' not found", name))?;

        let updated = BackendInfo {
            name: existing.name,
            backend_type: existing.backend_type,
            version: new_version,
            path: new_binary_path,
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
            gpu_type: existing.gpu_type,
            source: new_source,
        };

        self.add(updated)
    }

    /// List ALL versions of a backend (active + inactive), ordered by installed_at DESC.
    ///
    /// Returns Ok(None) if no backend with that name exists at all.
    pub fn list_all_versions(&self, name: &str) -> Result<Option<Vec<BackendInfo>>> {
        let records = list_backend_versions(&self.conn, name)
            .with_context(|| format!("Failed to query versions for backend '{}'", name))?;

        if records.is_empty() {
            return Ok(None);
        }

        records
            .into_iter()
            .map(Self::record_to_backend_info)
            .collect::<Result<Vec<_>>>()
            .map(Some)
    }

    /// Activate a specific version of a backend.
    ///
    /// Deactivates all other versions and activates the requested one.
    /// Returns Ok(true) if the version was found and activated, Ok(false) if not found.
    pub fn activate(&mut self, name: &str, version: &str) -> Result<bool> {
        activate_backend_version(&self.conn, name, version).with_context(|| {
            format!(
                "Failed to activate backend '{}' version '{}'",
                name, version
            )
        })
    }

    /// Remove a single (name, version) installation from the registry.
    ///
    /// **Note:** This method handles **DB operations only** — it does NOT delete files from disk.
    /// File deletion is the caller's responsibility (e.g., in the CLI command).
    ///
    /// If this was the active version and other versions remain, the newest remaining
    /// version is activated. If this was the last version, the row is simply deleted
    /// (no active version remains for that backend name).
    pub fn remove_version(&mut self, name: &str, version: &str) -> Result<()> {
        // Get the record before deleting, to check if it was active and to get the path
        let record = get_backend_by_version(&self.conn, name, version)
            .with_context(|| format!("Failed to query backend '{}' version '{}'", name, version))?;

        let was_active = record.as_ref().is_some_and(|r| r.is_active);

        // Delete the DB row
        delete_backend_installation(&self.conn, name, version).with_context(|| {
            format!("Failed to remove backend '{}' version '{}'", name, version)
        })?;

        // If this was the active version, we need to activate another one if available
        if was_active {
            let remaining = list_backend_versions(&self.conn, name).with_context(|| {
                format!("Failed to query remaining versions for backend '{}'", name)
            })?;

            if !remaining.is_empty() {
                // Activate the newest remaining version (first in DESC order)
                let newest = &remaining[0];
                activate_backend_version(&self.conn, name, &newest.version).with_context(|| {
                    format!(
                        "Failed to activate fallback version '{}' for backend '{}'",
                        newest.version, name
                    )
                })?;
            }
        }

        Ok(())
    }
}

impl BackendRegistry {
    /// Convert a `BackendInstallationRecord` to a `BackendInfo`.
    fn record_to_backend_info(record: BackendInstallationRecord) -> Result<BackendInfo> {
        let backend_type: BackendType = record
            .backend_type
            .parse()
            .map_err(|e: String| anyhow!("{}", e))?;

        let gpu_type: Option<GpuType> = match record.gpu_type {
            Some(ref s) => Some(
                serde_json::from_str(s)
                    .with_context(|| format!("Failed to deserialize gpu_type: {}", s))?,
            ),
            None => None,
        };

        let source: Option<BackendSource> = match record.source {
            Some(ref s) => Some(
                serde_json::from_str(s)
                    .with_context(|| format!("Failed to deserialize source: {}", s))?,
            ),
            None => None,
        };

        Ok(BackendInfo {
            name: record.name,
            backend_type,
            version: record.version,
            path: PathBuf::from(&record.path),
            installed_at: record.installed_at,
            gpu_type,
            source,
        })
    }

    /// Convert a `BackendInfo` to a `BackendInstallationRecord`.
    fn backend_info_to_record(backend: &BackendInfo) -> Result<BackendInstallationRecord> {
        let gpu_type_json: Option<String> = match &backend.gpu_type {
            Some(g) => {
                Some(serde_json::to_string(g).with_context(|| "Failed to serialize gpu_type")?)
            }
            None => None,
        };

        let source_json: Option<String> = match &backend.source {
            Some(s) => {
                Some(serde_json::to_string(s).with_context(|| "Failed to serialize source")?)
            }
            None => None,
        };

        Ok(BackendInstallationRecord {
            id: 0,
            name: backend.name.clone(),
            backend_type: backend.backend_type.to_string(),
            version: backend.version.clone(),
            path: backend.path.to_string_lossy().to_string(),
            installed_at: backend.installed_at,
            gpu_type: gpu_type_json,
            source: source_json,
            is_active: true,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackendType {
    LlamaCpp,
    IkLlama,
    Custom,
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::LlamaCpp => write!(f, "llama_cpp"),
            BackendType::IkLlama => write!(f, "ik_llama"),
            BackendType::Custom => write!(f, "custom"),
        }
    }
}

impl FromStr for BackendType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "llama_cpp" | "llamacpp" => Ok(BackendType::LlamaCpp),
            "ik_llama" | "ik-llama" | "ikllama" => Ok(BackendType::IkLlama),
            "custom" => Ok(BackendType::Custom),
            _ => Err(format!(
                "Unknown backend type '{}'. Supported: llama_cpp, ik_llama, custom",
                s
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backend_info(name: &str, version: &str) -> BackendInfo {
        BackendInfo {
            name: name.to_string(),
            backend_type: BackendType::LlamaCpp,
            version: version.to_string(),
            path: PathBuf::from(format!("/path/to/{}", name)),
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            gpu_type: None,
            source: None,
        }
    }

    #[test]
    fn test_registry_add_and_list() {
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        registry
            .add(make_backend_info("llama_cpp", "b8407"))
            .unwrap();

        let backends = registry.list().unwrap();
        assert_eq!(backends.len(), 1);
        assert_eq!(backends[0].name, "llama_cpp");
    }

    #[test]
    fn test_registry_remove() {
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        registry
            .add(make_backend_info("llama_cpp", "b8407"))
            .unwrap();

        registry.remove("llama_cpp").unwrap();

        let backends = registry.list().unwrap();
        assert_eq!(backends.len(), 0);
    }

    #[test]
    fn test_registry_update_version() {
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        registry
            .add(make_backend_info("llama_cpp", "b8407"))
            .unwrap();

        registry
            .update_version(
                "llama_cpp",
                "b9000".to_string(),
                PathBuf::from("/path/to/llama_cpp_v2"),
                None,
            )
            .unwrap();

        let backend = registry.get("llama_cpp").unwrap().unwrap();
        assert_eq!(backend.version, "b9000");
    }

    #[test]
    fn test_registry_get_returns_none_for_unknown() {
        let registry = BackendRegistry::open_in_memory().unwrap();
        let result = registry.get("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_registry_list_all_versions() {
        let registry = BackendRegistry::open_in_memory().unwrap();

        // No versions for unknown backend
        assert!(registry.list_all_versions("nonexistent").unwrap().is_none());

        // Add two versions of the same backend with distinct timestamps
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let info1 = BackendInfo {
            name: "llama_cpp".to_string(),
            backend_type: BackendType::LlamaCpp,
            version: "b8407".to_string(),
            path: PathBuf::from("/path/to/llama_cpp"),
            installed_at: now - 100,
            gpu_type: None,
            source: None,
        };

        let info2 = BackendInfo {
            name: "llama_cpp".to_string(),
            backend_type: BackendType::LlamaCpp,
            version: "b9000".to_string(),
            path: PathBuf::from("/path/to/llama_cpp"),
            installed_at: now,
            gpu_type: None,
            source: None,
        };

        registry.add(info1).unwrap();
        registry.add(info2).unwrap();

        let versions = registry.list_all_versions("llama_cpp").unwrap().unwrap();
        assert_eq!(versions.len(), 2);
        // Newest should be first (order by installed_at DESC)
        assert_eq!(versions[0].version, "b9000");
        assert_eq!(versions[1].version, "b8407");
    }

    #[test]
    fn test_registry_activate_version() {
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        registry
            .add(make_backend_info("llama_cpp", "b8407"))
            .unwrap();
        registry
            .add(make_backend_info("llama_cpp", "b9000"))
            .unwrap();

        // b9000 is active (added last)
        let active = registry.get("llama_cpp").unwrap().unwrap();
        assert_eq!(active.version, "b9000");

        // Activate b8407
        let result = registry.activate("llama_cpp", "b8407").unwrap();
        assert!(result);

        // Now b8407 should be active
        let active = registry.get("llama_cpp").unwrap().unwrap();
        assert_eq!(active.version, "b8407");
    }

    #[test]
    fn test_registry_activate_nonexistent_version() {
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        registry
            .add(make_backend_info("llama_cpp", "b8407"))
            .unwrap();

        let result = registry.activate("llama_cpp", "nonexistent").unwrap();
        assert!(!result);

        // Existing version should still be active
        let active = registry.get("llama_cpp").unwrap().unwrap();
        assert_eq!(active.version, "b8407");
    }

    #[test]
    fn test_registry_remove_version() {
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        registry
            .add(make_backend_info("llama_cpp", "b8407"))
            .unwrap();
        registry
            .add(make_backend_info("llama_cpp", "b9000"))
            .unwrap();

        // Both versions exist
        let all = registry.list_all_versions("llama_cpp").unwrap().unwrap();
        assert_eq!(all.len(), 2);

        // Remove b8407
        registry.remove_version("llama_cpp", "b8407").unwrap();

        // Only b9000 remains and should be active
        let all = registry.list_all_versions("llama_cpp").unwrap().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, "b9000");

        let active = registry.get("llama_cpp").unwrap().unwrap();
        assert_eq!(active.version, "b9000");
    }

    #[test]
    fn test_registry_remove_last_version_deactivates_others() {
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        registry
            .add(make_backend_info("llama_cpp", "b8407"))
            .unwrap();
        registry
            .add(make_backend_info("llama_cpp", "b9000"))
            .unwrap();

        // Remove the active one (b9000) — b8407 should become active
        registry.remove_version("llama_cpp", "b9000").unwrap();

        let all = registry.list_all_versions("llama_cpp").unwrap().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, "b8407");
    }

    #[test]
    fn test_registry_remove_last_version_cleans_up() {
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        registry
            .add(make_backend_info("llama_cpp", "b8407"))
            .unwrap();

        // Remove the only version
        registry.remove_version("llama_cpp", "b8407").unwrap();

        // No versions remain
        assert!(registry.list_all_versions("llama_cpp").unwrap().is_none());

        // list() returns empty for this backend
        let active = registry.get("llama_cpp").unwrap();
        assert!(active.is_none());
    }
}
