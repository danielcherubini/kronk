use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;

use crate::backends::types::{BackendInfo, BackendSource, BackendType};

/// A single backend option for UI dropdowns (e.g. model editor backend selector).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct BackendOption {
    pub name: String,
    #[serde(default)]
    pub variant: Option<String>,
    pub label: String,
}

/// Centralized backend data access. Each caller opens its own instance.
/// `Connection` is `Send` but not `Sync` — do not share across threads.
pub struct BackendManager {
    conn: Connection,
}

impl BackendManager {
    /// Open from config directory. Runs DB migrations on first open.
    /// Also runs legacy backend migration (idempotent, no-op if already done).
    pub fn open(config_dir: &std::path::Path) -> Result<Self> {
        let open_result = crate::db::open(config_dir)?;

        // Run legacy backend migration (idempotent, no-op if already done)
        let backends_dir = config_dir.join("backends");
        crate::backends::migration::migrate_legacy_backends(&open_result.conn, &backends_dir)
            .context("Failed to run legacy backend migration")?;

        Ok(Self {
            conn: open_result.conn,
        })
    }

    /// Open an in-memory database for testing.
    pub fn open_in_memory() -> Result<Self> {
        let open_result = crate::db::open_in_memory()?;
        Ok(Self {
            conn: open_result.conn,
        })
    }

    // ── Config (backend_configs table) ──────────────────────────

    /// Get config for a backend name + gpu_variant pair.
    pub fn get_config(
        &self,
        name: &str,
        gpu_variant: &str,
    ) -> Result<Option<crate::db::queries::BackendConfigRecord>> {
        crate::db::queries::get_backend_config(&self.conn, name, gpu_variant)
    }

    /// Insert or update config. Returns the row's integer id.
    pub fn save_config(
        &self,
        name: &str,
        gpu_variant: &str,
        default_args: &[String],
        health_check_url: Option<&str>,
    ) -> Result<i64> {
        crate::db::queries::upsert_backend_config(
            &self.conn,
            name,
            gpu_variant,
            default_args,
            health_check_url,
        )
    }

    /// List all backend config rows.
    pub fn list_configs(&self) -> Result<Vec<crate::db::queries::BackendConfigRecord>> {
        crate::db::queries::list_backend_configs(&self.conn)
    }

    // ── Resolution ───────────────────────────────────────────────

    /// Get default_args for a backend + variant from backend_configs.
    /// Returns empty vec if no config exists.
    pub fn get_default_args(&self, name: &str, gpu_variant: &str) -> Vec<String> {
        crate::db::queries::get_backend_config(&self.conn, name, gpu_variant)
            .ok()
            .flatten()
            .map(|c| c.default_args)
            .unwrap_or_default()
    }

    /// Get health_check_url from backend_configs.
    pub fn get_health_check_url(&self, name: &str, gpu_variant: &str) -> Option<String> {
        crate::db::queries::get_backend_config(&self.conn, name, gpu_variant)
            .ok()
            .flatten()
            .and_then(|c| c.health_check_url)
    }

    // ── Discovery ───────────────────────────────────────────────

    /// Return backend options for UI dropdowns (name, variant, label).
    /// Discovers from active installations in `backend_installations` table.
    pub fn available_backends(&self) -> Result<Vec<BackendOption>> {
        let active = crate::db::queries::list_active_backends(&self.conn)?;
        let mut seen = std::collections::HashSet::new();
        let mut options = Vec::new();
        for record in &active {
            let key = (record.name.clone(), record.gpu_variant.clone());
            if seen.insert(key.clone()) {
                options.push(BackendOption {
                    name: key.0.clone(),
                    variant: Some(key.1.clone()),
                    label: if key.1 == "cpu" {
                        key.0.clone()
                    } else {
                        format!("{} ({})", key.0, key.1)
                    },
                });
            }
        }
        Ok(options)
    }

    // ── Installation (backend_installations table) ─────────────

    /// Add a new backend installation, marking it as the active version.
    /// Delegates to `insert_backend_installation` which handles INSERT OR REPLACE
    /// and deactivates other versions of the same (name, gpu_variant).
    pub fn add_installation(&self, info: &BackendInfo) -> Result<()> {
        let record = Self::info_to_record(info)?;
        crate::db::queries::insert_backend_installation(&self.conn, &record)
            .with_context(|| format!("Failed to insert backend '{}'", info.name))
    }

    /// Get the active installation for a name + variant.
    pub fn get_active(&self, name: &str, gpu_variant: &str) -> Result<Option<BackendInfo>> {
        let record = crate::db::queries::get_active_backend(&self.conn, name, gpu_variant)?;
        match record {
            Some(r) => Ok(Some(Self::record_to_info(r)?)),
            None => Ok(None),
        }
    }

    /// List all active backend installations (one per name+variant).
    pub fn list_active(&self) -> Result<Vec<BackendInfo>> {
        let records = crate::db::queries::list_active_backends(&self.conn)?;
        records.into_iter().map(Self::record_to_info).collect()
    }

    /// List all versions of a backend.
    /// If `gpu_variant` is Some, filters to that variant. If None, returns all variants.
    /// Returns None if no versions exist for this name.
    pub fn list_versions(
        &self,
        name: &str,
        gpu_variant: Option<&str>,
    ) -> Result<Option<Vec<BackendInfo>>> {
        let records = crate::db::queries::list_backend_versions(&self.conn, name, gpu_variant)?;
        if records.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                records
                    .into_iter()
                    .map(Self::record_to_info)
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
    }

    /// Get a specific installation by (name, gpu_variant, version).
    pub fn get_by_version(
        &self,
        name: &str,
        gpu_variant: &str,
        version: &str,
    ) -> Result<Option<BackendInfo>> {
        let record =
            crate::db::queries::get_backend_by_version(&self.conn, name, gpu_variant, version)?;
        match record {
            Some(r) => Ok(Some(Self::record_to_info(r)?)),
            None => Ok(None),
        }
    }

    /// Activate a specific version for a name + variant.
    /// Deactivates all other versions of the same (name, gpu_variant).
    /// Returns true if the version was found and activated.
    pub fn activate(&self, name: &str, gpu_variant: &str, version: &str) -> Result<bool> {
        crate::db::queries::activate_backend_version(&self.conn, name, gpu_variant, version)
    }

    /// Update an existing backend to a new version (convenience).
    /// Reads the current active installation, builds a new BackendInfo with
    /// updated version/path/source, and calls add_installation.
    pub fn update_version(
        &self,
        name: &str,
        gpu_variant: &str,
        new_version: String,
        new_path: std::path::PathBuf,
        new_source: Option<BackendSource>,
    ) -> Result<()> {
        let existing = self
            .get_active(name, gpu_variant)?
            .ok_or_else(|| anyhow!("Backend '{}' variant '{}' not found", name, gpu_variant))?;
        let updated = BackendInfo {
            name: existing.name,
            backend_type: existing.backend_type,
            version: new_version,
            path: new_path,
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
            gpu_type: existing.gpu_type,
            gpu_variant: existing.gpu_variant,
            source: new_source,
        };
        self.add_installation(&updated)
    }

    /// Delete a specific (name, gpu_variant, version) installation row.
    /// If the deleted version was active, re-activates the newest remaining version.
    pub fn remove_version(&self, name: &str, gpu_variant: &str, version: &str) -> Result<()> {
        // Check if the target version exists before deleting
        let existing =
            crate::db::queries::get_backend_by_version(&self.conn, name, gpu_variant, version)?;
        let was_active = existing.as_ref().map(|r| r.is_active).unwrap_or(false);

        crate::db::queries::delete_backend_installation(&self.conn, name, gpu_variant, version)?;

        // If we deleted the active version, activate the newest remaining one
        if was_active {
            let remaining =
                crate::db::queries::list_backend_versions(&self.conn, name, Some(gpu_variant))?;
            if let Some(newest) = remaining.first() {
                crate::db::queries::activate_backend_version(
                    &self.conn,
                    name,
                    gpu_variant,
                    &newest.version,
                )?;
            }
        }

        Ok(())
    }

    /// Delete all versions of a backend.
    /// If `gpu_variant` is Some, only deletes that variant.
    /// If None, deletes all variants.
    pub fn delete_all_versions(&self, name: &str, gpu_variant: Option<&str>) -> Result<()> {
        crate::db::queries::delete_all_backend_versions(&self.conn, name, gpu_variant)
    }

    // ── Private helpers ──────────────────────────────────────

    fn info_to_record(info: &BackendInfo) -> Result<crate::db::queries::BackendInstallationRecord> {
        let gpu_type_json = info
            .gpu_type
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("Failed to serialize gpu_type")?;
        let source_json = info
            .source
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("Failed to serialize source")?;
        Ok(crate::db::queries::BackendInstallationRecord {
            id: 0,
            name: info.name.clone(),
            backend_type: info.backend_type.to_string(),
            version: info.version.clone(),
            path: info.path.to_string_lossy().to_string(),
            installed_at: info.installed_at,
            gpu_type: gpu_type_json,
            gpu_variant: info.gpu_variant.clone(),
            source: source_json,
            is_active: true,
        })
    }

    fn record_to_info(
        record: crate::db::queries::BackendInstallationRecord,
    ) -> Result<BackendInfo> {
        let gpu_type = record
            .gpu_type
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .context("Failed to deserialize gpu_type")?;
        let source = record
            .source
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .context("Failed to deserialize source")?;
        Ok(BackendInfo {
            name: record.name,
            backend_type: record.backend_type.parse().unwrap_or(BackendType::LlamaCpp),
            version: record.version,
            path: std::path::PathBuf::from(record.path),
            installed_at: record.installed_at,
            gpu_type,
            gpu_variant: record.gpu_variant,
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::BackendInstallationRecord;

    fn insert_active_backend(
        conn: &Connection,
        name: &str,
        gpu_variant: &str,
        version: &str,
    ) -> Result<()> {
        crate::db::queries::insert_backend_installation(
            conn,
            &BackendInstallationRecord {
                id: 0,
                name: name.to_string(),
                backend_type: "llama_cpp".to_string(),
                version: version.to_string(),
                path: "/tmp/test/llama-server".to_string(),
                installed_at: 0,
                gpu_type: None,
                gpu_variant: gpu_variant.to_string(),
                source: None,
                is_active: true,
            },
        )
    }

    #[test]
    fn test_open_in_memory_creates_instance() {
        let manager = BackendManager::open_in_memory().unwrap();
        // Should be able to call methods without panic
        let configs = manager.list_configs().unwrap();
        assert!(configs.is_empty());
    }

    #[test]
    fn test_save_and_get_config_roundtrip() {
        let manager = BackendManager::open_in_memory().unwrap();

        let args = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        let id = manager
            .save_config(
                "llama_cpp",
                "cpu",
                &args,
                Some("http://localhost:8080/health"),
            )
            .unwrap();
        assert_eq!(id, 1);

        let record = manager.get_config("llama_cpp", "cpu").unwrap().unwrap();
        assert_eq!(record.name, "llama_cpp");
        assert_eq!(record.gpu_variant, "cpu");
        assert_eq!(record.default_args, args);
        assert_eq!(
            record.health_check_url,
            Some("http://localhost:8080/health".to_string())
        );
    }

    #[test]
    fn test_save_config_updates_existing() {
        let manager = BackendManager::open_in_memory().unwrap();

        // Insert initial row
        let id1 = manager
            .save_config(
                "llama_cpp",
                "cpu",
                &["-fa 1".to_string()],
                Some("http://localhost:8080/health"),
            )
            .unwrap();

        // Upsert with different values
        let id2 = manager
            .save_config(
                "llama_cpp",
                "cpu",
                &["-fa 1".to_string(), "-b 2048".to_string()],
                Some("http://localhost:9090/health"),
            )
            .unwrap();

        // ID should be the same (updated, not re-inserted)
        assert_eq!(id1, id2);

        let record = manager.get_config("llama_cpp", "cpu").unwrap().unwrap();
        assert_eq!(record.default_args, vec!["-fa 1", "-b 2048"]);
        assert_eq!(
            record.health_check_url,
            Some("http://localhost:9090/health".to_string())
        );
    }

    #[test]
    fn test_get_config_returns_none_for_missing() {
        let manager = BackendManager::open_in_memory().unwrap();
        let result = manager.get_config("nonexistent", "cpu").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_configs_returns_all() {
        let manager = BackendManager::open_in_memory().unwrap();

        manager
            .save_config(
                "llama_cpp",
                "cpu",
                &["-fa 1".to_string()],
                Some("http://localhost:8080/health"),
            )
            .unwrap();
        manager
            .save_config("llama_cpp", "vulkan", &[], None)
            .unwrap();
        manager.save_config("ik_llama", "cpu", &[], None).unwrap();

        let configs = manager.list_configs().unwrap();
        assert_eq!(configs.len(), 3);

        let cpu = configs
            .iter()
            .find(|c| c.name == "llama_cpp" && c.gpu_variant == "cpu")
            .unwrap();
        assert_eq!(cpu.default_args, vec!["-fa 1"]);

        let vulkan = configs
            .iter()
            .find(|c| c.name == "llama_cpp" && c.gpu_variant == "vulkan")
            .unwrap();
        assert!(vulkan.default_args.is_empty());
        assert!(vulkan.health_check_url.is_none());
    }

    #[test]
    fn test_available_backends_returns_options() {
        let manager = BackendManager::open_in_memory().unwrap();

        insert_active_backend(&manager.conn, "llama_cpp", "cpu", "b8407").unwrap();
        insert_active_backend(&manager.conn, "ik_llama", "cuda", "main").unwrap();

        let options = manager.available_backends().unwrap();
        assert_eq!(options.len(), 2);

        let llama = options.iter().find(|o| o.name == "llama_cpp").unwrap();
        assert_eq!(llama.variant, Some("cpu".to_string()));
        assert_eq!(llama.label, "llama_cpp");

        let ik = options.iter().find(|o| o.name == "ik_llama").unwrap();
        assert_eq!(ik.variant, Some("cuda".to_string()));
        assert_eq!(ik.label, "ik_llama (cuda)");
    }

    #[test]
    fn test_available_backends_groups_by_variant() {
        let manager = BackendManager::open_in_memory().unwrap();

        // Insert multiple versions of the same backend+variant
        insert_active_backend(&manager.conn, "llama_cpp", "cpu", "b8407").unwrap();
        insert_active_backend(&manager.conn, "llama_cpp", "cpu", "b9000").unwrap();
        insert_active_backend(&manager.conn, "llama_cpp", "cuda", "b8407").unwrap();

        let options = manager.available_backends().unwrap();
        // Should have 2 options: one for cpu, one for cuda (not 3)
        assert_eq!(options.len(), 2);

        let cpu_opt = options
            .iter()
            .find(|o| o.variant == Some("cpu".to_string()))
            .unwrap();
        assert_eq!(cpu_opt.name, "llama_cpp");

        let cuda_opt = options
            .iter()
            .find(|o| o.variant == Some("cuda".to_string()))
            .unwrap();
        assert_eq!(cuda_opt.name, "llama_cpp");
    }

    // ── Installation tests ───────────────────────────────────────────

    fn make_backend_info(name: &str, version: &str) -> BackendInfo {
        BackendInfo {
            name: name.to_string(),
            backend_type: BackendType::LlamaCpp,
            version: version.to_string(),
            path: std::path::PathBuf::from(format!("/path/to/{}", name)),
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            gpu_type: None,
            gpu_variant: "cpu".to_string(),
            source: None,
        }
    }

    #[test]
    fn test_add_and_get_installation() {
        let manager = BackendManager::open_in_memory().unwrap();

        let info = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info).unwrap();

        let result = manager.get_active("llama_cpp", "cpu").unwrap().unwrap();
        assert_eq!(result.name, "llama_cpp");
        assert_eq!(result.version, "b8407");
        assert_eq!(result.backend_type, BackendType::LlamaCpp);
    }

    #[test]
    fn test_add_installation_replaces_old() {
        let manager = BackendManager::open_in_memory().unwrap();

        let info1 = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info1).unwrap();

        // Add a new version — should deactivate old one
        let info2 = BackendInfo {
            version: "b9000".to_string(),
            installed_at: info1.installed_at + 100,
            ..info1.clone()
        };
        manager.add_installation(&info2).unwrap();

        // Active should be the new version
        let active = manager.get_active("llama_cpp", "cpu").unwrap().unwrap();
        assert_eq!(active.version, "b9000");

        // Old version should still exist but not be active
        let old = manager
            .get_by_version("llama_cpp", "cpu", "b8407")
            .unwrap()
            .unwrap();
        assert_eq!(old.version, "b8407");
    }

    #[test]
    fn test_list_active_returns_all() {
        let manager = BackendManager::open_in_memory().unwrap();

        manager
            .add_installation(&make_backend_info("llama_cpp", "b8407"))
            .unwrap();
        let ik_info = BackendInfo {
            name: "ik_llama".to_string(),
            backend_type: BackendType::IkLlama,
            gpu_variant: "cuda".to_string(),
            ..make_backend_info("ik_llama", "main")
        };
        manager.add_installation(&ik_info).unwrap();

        let active = manager.list_active().unwrap();
        assert_eq!(active.len(), 2);

        let llama = active.iter().find(|b| b.name == "llama_cpp").unwrap();
        assert_eq!(llama.version, "b8407");

        let ik = active.iter().find(|b| b.name == "ik_llama").unwrap();
        assert_eq!(ik.version, "main");
    }

    #[test]
    fn test_list_versions_by_variant() {
        let manager = BackendManager::open_in_memory().unwrap();

        let info1 = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info1).unwrap();

        let info2 = BackendInfo {
            version: "b9000".to_string(),
            installed_at: info1.installed_at + 100,
            ..info1.clone()
        };
        manager.add_installation(&info2).unwrap();

        let info_cuda = BackendInfo {
            name: "llama_cpp".to_string(),
            backend_type: BackendType::LlamaCpp,
            version: "b8407".to_string(),
            path: std::path::PathBuf::from("/path/to/llama_cpp"),
            installed_at: info1.installed_at,
            gpu_type: None,
            gpu_variant: "cuda".to_string(),
            source: None,
        };
        manager.add_installation(&info_cuda).unwrap();

        // Filter by cpu variant
        let versions = manager
            .list_versions("llama_cpp", Some("cpu"))
            .unwrap()
            .unwrap();
        assert_eq!(versions.len(), 2);

        // Filter by cuda variant
        let versions = manager
            .list_versions("llama_cpp", Some("cuda"))
            .unwrap()
            .unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].gpu_variant, "cuda");

        // No variant filter — should return all
        let all_versions = manager.list_versions("llama_cpp", None).unwrap().unwrap();
        assert_eq!(all_versions.len(), 3);

        // Unknown backend returns None
        assert!(manager
            .list_versions("nonexistent", None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_activate_switches_active() {
        let manager = BackendManager::open_in_memory().unwrap();

        let info1 = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info1).unwrap();

        let info2 = BackendInfo {
            version: "b9000".to_string(),
            installed_at: info1.installed_at + 100,
            ..info1.clone()
        };
        manager.add_installation(&info2).unwrap();

        // b9000 should be active (added last)
        let active = manager.get_active("llama_cpp", "cpu").unwrap().unwrap();
        assert_eq!(active.version, "b9000");

        // Activate b8407
        let result = manager.activate("llama_cpp", "cpu", "b8407").unwrap();
        assert!(result);

        let active = manager.get_active("llama_cpp", "cpu").unwrap().unwrap();
        assert_eq!(active.version, "b8407");

        // Activate nonexistent version returns false
        let result = manager.activate("llama_cpp", "cpu", "nonexistent").unwrap();
        assert!(!result);

        // Active should still be b8407
        let active = manager.get_active("llama_cpp", "cpu").unwrap().unwrap();
        assert_eq!(active.version, "b8407");
    }

    #[test]
    fn test_remove_version_deletes_row() {
        let manager = BackendManager::open_in_memory().unwrap();

        let info1 = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info1).unwrap();

        let info2 = BackendInfo {
            version: "b9000".to_string(),
            installed_at: info1.installed_at + 100,
            ..info1.clone()
        };
        manager.add_installation(&info2).unwrap();

        // Two versions exist
        let all = manager.list_versions("llama_cpp", None).unwrap().unwrap();
        assert_eq!(all.len(), 2);

        // Remove b8407
        manager.remove_version("llama_cpp", "cpu", "b8407").unwrap();

        // Only b9000 remains
        let all = manager.list_versions("llama_cpp", None).unwrap().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, "b9000");

        // b9000 should be active
        let active = manager.get_active("llama_cpp", "cpu").unwrap().unwrap();
        assert_eq!(active.version, "b9000");
    }

    #[test]
    fn test_delete_all_versions_with_variant() {
        let manager = BackendManager::open_in_memory().unwrap();

        manager
            .add_installation(&make_backend_info("llama_cpp", "b8407"))
            .unwrap();

        let info_cuda = BackendInfo {
            gpu_variant: "cuda".to_string(),
            ..make_backend_info("llama_cpp", "b8407")
        };
        manager.add_installation(&info_cuda).unwrap();

        // Delete only cpu variant
        manager
            .delete_all_versions("llama_cpp", Some("cpu"))
            .unwrap();

        // CPU variant should be gone
        assert!(manager
            .list_versions("llama_cpp", Some("cpu"))
            .unwrap()
            .is_none());

        // CUDA variant should still exist
        let versions = manager
            .list_versions("llama_cpp", Some("cuda"))
            .unwrap()
            .unwrap();
        assert_eq!(versions.len(), 1);
    }

    #[test]
    fn test_delete_all_versions_without_variant_deletes_all() {
        let manager = BackendManager::open_in_memory().unwrap();

        manager
            .add_installation(&make_backend_info("llama_cpp", "b8407"))
            .unwrap();

        let info_cuda = BackendInfo {
            gpu_variant: "cuda".to_string(),
            ..make_backend_info("llama_cpp", "b8407")
        };
        manager.add_installation(&info_cuda).unwrap();

        // Delete all variants
        manager.delete_all_versions("llama_cpp", None).unwrap();

        // All versions should be gone
        assert!(manager.list_versions("llama_cpp", None).unwrap().is_none());
    }

    // ── Resolution tests ───────────────────────────────────────────

    #[test]
    fn test_get_default_args_returns_args() {
        let manager = BackendManager::open_in_memory().unwrap();

        let args = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        manager
            .save_config("llama_cpp", "cpu", &args, None)
            .unwrap();

        let result = manager.get_default_args("llama_cpp", "cpu");
        assert_eq!(result, args);
    }

    #[test]
    fn test_get_default_args_returns_empty_for_missing() {
        let manager = BackendManager::open_in_memory().unwrap();

        let result = manager.get_default_args("nonexistent", "cpu");
        assert!(result.is_empty());
    }

    #[test]
    fn test_get_health_check_url_returns_url() {
        let manager = BackendManager::open_in_memory().unwrap();

        manager
            .save_config(
                "llama_cpp",
                "cpu",
                &[],
                Some("http://localhost:8080/health"),
            )
            .unwrap();

        let result = manager.get_health_check_url("llama_cpp", "cpu");
        assert_eq!(result, Some("http://localhost:8080/health".to_string()));
    }

    #[test]
    fn test_get_health_check_url_returns_none_for_missing() {
        let manager = BackendManager::open_in_memory().unwrap();

        let result = manager.get_health_check_url("nonexistent", "cpu");
        assert!(result.is_none());
    }

    #[test]
    fn test_update_version_convenience() {
        let manager = BackendManager::open_in_memory().unwrap();

        let info = make_backend_info("llama_cpp", "b8407");
        manager.add_installation(&info).unwrap();

        // Update to new version
        manager
            .update_version(
                "llama_cpp",
                "cpu",
                "b9000".to_string(),
                std::path::PathBuf::from("/path/to/llama_cpp_v2"),
                Some(BackendSource::Prebuilt {
                    version: "b9000".to_string(),
                }),
            )
            .unwrap();

        let active = manager.get_active("llama_cpp", "cpu").unwrap().unwrap();
        assert_eq!(active.version, "b9000");
        assert_eq!(
            active.path,
            std::path::PathBuf::from("/path/to/llama_cpp_v2")
        );
        assert!(active.source.is_some());
    }
}
