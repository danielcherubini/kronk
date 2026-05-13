use anyhow::{Context, Result};
use rusqlite::Connection;

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
}
