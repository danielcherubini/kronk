use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

use crate::config::ModelConfig;
use crate::db::queries::ModelConfigRecord;

/// Centralized model data access. Each caller opens its own instance.
/// `Connection` is `Send` but not `Sync` — do not share across threads.
pub struct ModelManager {
    conn: Connection,
}

impl ModelManager {
    /// Open from config directory. Runs DB migrations on first open.
    pub fn open(config_dir: &Path) -> Result<Self> {
        let open_result = crate::db::open(config_dir)?;
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

    /// Returns reference to the underlying connection.
    ///
    /// This is a permanent escape hatch for callers that need raw access:
    /// - Async functions that must not hold `&Connection` across `.await`
    /// - Transactional operations that need to create a `Transaction` directly
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Execute a closure within a transaction for atomic multi-step operations.
    pub fn transaction<F, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Transaction) -> Result<T>,
    {
        let tx = self.conn.transaction()?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    // ── Config CRUD ────────────────────────────────────────────

    /// Get the model configuration by id. Returns None if not found.
    pub fn get_config(&self, id: i64) -> Result<Option<ModelConfigRecord>> {
        crate::db::queries::get_model_config(&self.conn, id)
    }

    /// Get the model configuration by repo_id. Returns None if not found.
    pub fn get_config_by_repo_id(&self, repo_id: &str) -> Result<Option<ModelConfigRecord>> {
        crate::db::queries::get_model_config_by_repo_id(&self.conn, repo_id)
    }

    /// Get all stored model configurations.
    pub fn get_all_configs(&self) -> Result<Vec<ModelConfigRecord>> {
        crate::db::queries::get_all_model_configs(&self.conn)
    }

    /// Insert or update the model configuration. Returns the model id.
    pub fn upsert_config(&self, record: &ModelConfigRecord) -> Result<i64> {
        crate::db::queries::upsert_model_config(&self.conn, record)
    }

    /// Delete the model configuration by id. CASCADE deletes model_pulls and model_files.
    pub fn delete_config(&self, id: i64) -> Result<()> {
        crate::db::queries::delete_model_config(&self.conn, id)
    }

    /// Rename a config by updating its repo_id.
    /// Uses a direct UPDATE to avoid triggering CASCADE deletes on model_files.
    pub fn rename_config(&self, id: i64, new_repo_id: &str) -> Result<()> {
        // Verify the record exists
        let _exists = self
            .get_config(id)?
            .ok_or_else(|| anyhow::anyhow!("Model config with id {} not found", id))?;
        self.conn.execute(
            "UPDATE model_configs SET repo_id = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?2",
            rusqlite::params![new_repo_id, id],
        )?;
        Ok(())
    }

    /// Enable a model by config_key.
    pub fn enable_model(&self, config_key: &str) -> Result<()> {
        let mut configs = crate::db::load_model_configs(&self.conn)?;
        if let Some(mc) = configs.get_mut(config_key) {
            mc.enabled = true;
            let repo_id = crate::db::config_key_to_repo_id(config_key);
            let record = mc.to_db_record(&repo_id);
            crate::db::queries::upsert_model_config(&self.conn, &record)?;
        }
        Ok(())
    }

    /// Disable a model by config_key.
    pub fn disable_model(&self, config_key: &str) -> Result<()> {
        let mut configs = crate::db::load_model_configs(&self.conn)?;
        if let Some(mc) = configs.get_mut(config_key) {
            mc.enabled = false;
            let repo_id = crate::db::config_key_to_repo_id(config_key);
            let record = mc.to_db_record(&repo_id);
            crate::db::queries::upsert_model_config(&self.conn, &record)?;
        }
        Ok(())
    }

    /// Convenience method to save a ModelConfig as a DB record.
    ///
    /// Converts config_key to repo_id, converts ModelConfig → ModelConfigRecord,
    /// sets api_name default, and calls upsert_config.
    pub fn save_model_config(&self, config_key: &str, mc: &ModelConfig) -> Result<i64> {
        let repo_id = crate::db::config_key_to_repo_id(config_key);
        let mut record = mc.to_db_record(&repo_id);
        if record.api_name.as_deref().is_none_or(str::is_empty) {
            record.api_name = Some(repo_id.clone());
        }
        self.upsert_config(&record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_record(repo_id: &str) -> ModelConfigRecord {
        use chrono::{SecondsFormat, Utc};
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        ModelConfigRecord {
            id: 0,
            repo_id: repo_id.to_string(),
            display_name: Some("Test Model".to_string()),
            backend: "llama.cpp".to_string(),
            gpu_variant: None,
            enabled: true,
            selected_quant: None,
            selected_mmproj: None,
            context_length: None,
            num_parallel: None,
            kv_unified: false,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: Some(repo_id.to_string()),
            health_check: None,
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[test]
    fn test_open_in_memory() {
        let manager = ModelManager::open_in_memory().unwrap();
        let _conn = manager.conn();
        let configs = manager.get_all_configs().unwrap();
        assert!(configs.is_empty());
    }

    #[test]
    fn test_upsert_and_get_config() {
        let manager = ModelManager::open_in_memory().unwrap();
        let record = make_test_record("owner/test-repo");
        let id = manager.upsert_config(&record).unwrap();
        assert_eq!(id, 1);

        let fetched = manager.get_config(id).unwrap().unwrap();
        assert_eq!(fetched.repo_id, "owner/test-repo");
        assert_eq!(fetched.display_name, Some("Test Model".to_string()));

        let all = manager.get_all_configs().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_get_config_by_repo_id_missing() {
        let manager = ModelManager::open_in_memory().unwrap();
        let result = manager.get_config_by_repo_id("nonexistent/repo").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_enable_disable_model() {
        let manager = ModelManager::open_in_memory().unwrap();

        let mc = ModelConfig {
            backend: "llama.cpp".to_string(),
            enabled: true,
            ..Default::default()
        };
        manager.save_model_config("owner--test-repo", &mc).unwrap();

        // Disable it
        manager.disable_model("owner--test-repo").unwrap();
        let record = manager
            .get_config_by_repo_id("owner/test-repo")
            .unwrap()
            .unwrap();
        assert!(!record.enabled);

        // Re-enable it
        manager.enable_model("owner--test-repo").unwrap();
        let record = manager
            .get_config_by_repo_id("owner/test-repo")
            .unwrap()
            .unwrap();
        assert!(record.enabled);
    }

    #[test]
    fn test_rename_config() {
        let manager = ModelManager::open_in_memory().unwrap();
        let record = make_test_record("owner/old-name");
        let id = manager.upsert_config(&record).unwrap();

        manager.rename_config(id, "owner/new-name").unwrap();

        // Old repo_id should return None
        let old = manager.get_config_by_repo_id("owner/old-name").unwrap();
        assert!(old.is_none());

        // New repo_id should return the record
        let new = manager
            .get_config_by_repo_id("owner/new-name")
            .unwrap()
            .unwrap();
        assert_eq!(new.repo_id, "owner/new-name");
        assert_eq!(new.display_name, Some("Test Model".to_string()));
    }

    #[test]
    fn test_save_model_config_convenience() {
        let manager = ModelManager::open_in_memory().unwrap();

        let mc = ModelConfig {
            backend: "llama.cpp".to_string(),
            display_name: Some("My Model".to_string()),
            enabled: true,
            ..Default::default()
        };
        let id = manager.save_model_config("owner--my-model", &mc).unwrap();
        assert_eq!(id, 1);

        let record = manager.get_config(id).unwrap().unwrap();
        assert_eq!(record.repo_id, "owner/my-model");
        assert_eq!(record.backend, "llama.cpp");
        assert_eq!(record.display_name, Some("My Model".to_string()));
        assert_eq!(record.enabled, true);
        assert_eq!(record.api_name, Some("owner/my-model".to_string()));
    }
}
