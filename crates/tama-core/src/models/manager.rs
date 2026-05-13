use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

use crate::config::ModelConfig;
use crate::db::queries::{
    ActiveModelRecord, DownloadLogEntry, DownloadQueueItem, ModelConfigRecord, ModelFileRecord,
    ModelPullRecord, UpdateCheckParams, UpdateCheckRecord,
};

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

    // ── File tracking ──────────────────────────────────────────

    /// Get all stored file records for a model.
    pub fn get_files(&self, model_id: i64) -> Result<Vec<ModelFileRecord>> {
        crate::db::queries::get_model_files(&self.conn, model_id)
    }

    /// Get all stored file records across all models.
    pub fn get_all_files(&self) -> Result<Vec<ModelFileRecord>> {
        crate::db::queries::get_all_model_files(&self.conn)
    }

    /// Insert or update a file record for a downloaded GGUF.
    pub fn upsert_file(
        &self,
        model_id: i64,
        repo_id: &str,
        filename: &str,
        quant: Option<&str>,
        lfs_oid: Option<&str>,
        size_bytes: Option<i64>,
    ) -> Result<()> {
        crate::db::queries::upsert_model_file(
            &self.conn, model_id, repo_id, filename, quant, lfs_oid, size_bytes,
        )
    }

    /// Delete a single model file record by (model_id, filename).
    pub fn delete_file(&self, model_id: i64, filename: &str) -> Result<()> {
        crate::db::queries::delete_model_file(&self.conn, model_id, filename)
    }

    /// Update the verification columns for a single file.
    pub fn update_verification(
        &self,
        model_id: i64,
        filename: &str,
        verified_ok: Option<bool>,
        verify_error: Option<&str>,
    ) -> Result<()> {
        crate::db::queries::update_verification(
            &self.conn,
            model_id,
            filename,
            verified_ok,
            verify_error,
        )
    }

    // ── Pull tracking ──────────────────────────────────────────

    /// Insert or update the pull record for a model.
    pub fn upsert_pull(&self, model_id: i64, repo_id: &str, commit_sha: &str) -> Result<()> {
        crate::db::queries::upsert_model_pull(&self.conn, model_id, repo_id, commit_sha)
    }

    /// Get the stored pull record for a model. Returns None if never pulled.
    pub fn get_pull(&self, model_id: i64) -> Result<Option<ModelPullRecord>> {
        crate::db::queries::get_model_pull(&self.conn, model_id)
    }

    /// Log a download event (append-only).
    pub fn log_download(&self, entry: &DownloadLogEntry) -> Result<()> {
        crate::db::queries::log_download(&self.conn, entry)
    }

    // ── Active models ──────────────────────────────────────────

    /// Insert or replace an active model entry when a backend is loaded.
    pub fn insert_active(
        &self,
        server_name: &str,
        model_name: &str,
        backend: &str,
        pid: i64,
        port: i64,
        backend_url: &str,
    ) -> Result<()> {
        crate::db::queries::insert_active_model(
            &self.conn,
            server_name,
            model_name,
            backend,
            pid,
            port,
            backend_url,
        )
    }

    /// Remove an active model entry when a backend is unloaded.
    pub fn remove_active(&self, server_name: &str) -> Result<()> {
        crate::db::queries::remove_active_model(&self.conn, server_name)
    }

    /// Get all active model entries (for status / cleanup).
    pub fn get_active(&self) -> Result<Vec<ActiveModelRecord>> {
        crate::db::queries::get_active_models(&self.conn)
    }

    /// Rename an active model by updating its primary key (server_name).
    pub fn rename_active(&self, old_name: &str, new_name: &str) -> Result<()> {
        crate::db::queries::rename_active_model(&self.conn, old_name, new_name)
    }

    // ── Download queue ─────────────────────────────────────────

    /// Insert a new item into the download queue. Returns the new row id.
    #[allow(clippy::too_many_arguments)]
    pub fn queue_insert(
        &self,
        job_id: &str,
        repo_id: &str,
        filename: &str,
        display_name: Option<&str>,
        kind: &str,
        quant: Option<&str>,
        context_length: Option<u32>,
    ) -> Result<i64> {
        crate::db::queries::insert_queue_item(
            &self.conn,
            job_id,
            repo_id,
            filename,
            display_name,
            kind,
            quant,
            context_length,
        )
    }

    /// Retrieve the oldest queued item (FIFO).
    pub fn queue_get_queued(&self) -> Result<Option<DownloadQueueItem>> {
        crate::db::queries::get_queued_item(&self.conn)
    }

    /// Get all active items (queued, running, verifying), ordered by status priority then queued_at.
    pub fn queue_get_active(&self) -> Result<Vec<DownloadQueueItem>> {
        crate::db::queries::get_active_items(&self.conn)
    }

    /// Get history items (completed, failed, cancelled), sorted newest first.
    pub fn queue_get_history(&self, limit: i64, offset: i64) -> Result<Vec<DownloadQueueItem>> {
        crate::db::queries::get_history_items(&self.conn, limit, offset)
    }

    /// Update a queue item's status and related fields.
    pub fn queue_update_status(
        &self,
        job_id: &str,
        new_status: &str,
        bytes_downloaded: i64,
        total_bytes: Option<i64>,
        error_message: Option<&str>,
    ) -> Result<()> {
        crate::db::queries::update_queue_status(
            &self.conn,
            job_id,
            new_status,
            bytes_downloaded,
            total_bytes,
            error_message,
        )
    }

    /// Cancel a queue item if it hasn't reached a terminal state.
    pub fn queue_cancel(&self, job_id: &str) -> Result<()> {
        crate::db::queries::cancel_queue_item(&self.conn, job_id)
    }

    /// Retrieve a queue item by its job_id.
    pub fn queue_get_by_job_id(&self, job_id: &str) -> Result<Option<DownloadQueueItem>> {
        crate::db::queries::get_item_by_job_id(&self.conn, job_id)
    }

    // ── Update checks ──────────────────────────────────────────

    /// Get a stored update check record.
    pub fn get_update_check(
        &self,
        item_type: &str,
        item_id: &str,
    ) -> Result<Option<UpdateCheckRecord>> {
        crate::db::queries::get_update_check(&self.conn, item_type, item_id)
    }

    /// Insert or update an update check record.
    pub fn upsert_update_check(&self, params: UpdateCheckParams) -> Result<()> {
        crate::db::queries::upsert_update_check(&self.conn, params)
    }

    /// Delete a stored update check record.
    pub fn delete_update_check(&self, item_type: &str, item_id: &str) -> Result<()> {
        crate::db::queries::delete_update_check(&self.conn, item_type, item_id)
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
    fn test_file_operations() {
        let manager = ModelManager::open_in_memory().unwrap();

        // Insert a model config first (required for FK)
        let record = make_test_record("owner/test-model");
        let model_id = manager.upsert_config(&record).unwrap();

        // Verify no files initially
        let files = manager.get_files(model_id).unwrap();
        assert!(files.is_empty());

        // Upsert a file
        manager
            .upsert_file(
                model_id,
                "owner/test-model",
                "test-model.Q4_K_M.gguf",
                Some("Q4_K_M"),
                Some("sha256-abc123"),
                Some(1_000_000),
            )
            .unwrap();

        // Verify it appears in get_files
        let files = manager.get_files(model_id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "test-model.Q4_K_M.gguf");
        assert_eq!(files[0].quant, Some("Q4_K_M".to_string()));
        assert_eq!(files[0].lfs_oid, Some("sha256-abc123".to_string()));
        assert_eq!(files[0].size_bytes, Some(1_000_000));

        // Verify it appears in get_all_files
        let all_files = manager.get_all_files().unwrap();
        assert_eq!(all_files.len(), 1);

        // Update verification
        manager
            .update_verification(model_id, "test-model.Q4_K_M.gguf", Some(true), None)
            .unwrap();

        let files = manager.get_files(model_id).unwrap();
        assert_eq!(files[0].verified_ok, Some(true));

        // Delete the file
        manager
            .delete_file(model_id, "test-model.Q4_K_M.gguf")
            .unwrap();

        let files = manager.get_files(model_id).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_pull_operations() {
        let manager = ModelManager::open_in_memory().unwrap();

        // Insert a model config
        let record = make_test_record("owner/test-model");
        let model_id = manager.upsert_config(&record).unwrap();

        // No pull record initially
        let pull = manager.get_pull(model_id).unwrap();
        assert!(pull.is_none());

        // Upsert a pull record
        manager
            .upsert_pull(model_id, "owner/test-model", "abc123def456")
            .unwrap();

        // Verify pull record
        let pull = manager.get_pull(model_id).unwrap().unwrap();
        assert_eq!(pull.model_id, model_id);
        assert_eq!(pull.repo_id, "owner/test-model");
        assert_eq!(pull.commit_sha, "abc123def456");
    }

    #[test]
    fn test_log_download() {
        let manager = ModelManager::open_in_memory().unwrap();

        let entry = DownloadLogEntry {
            repo_id: "owner/test-model".to_string(),
            filename: "test.gguf".to_string(),
            started_at: "2025-01-01T00:00:00Z".to_string(),
            completed_at: Some("2025-01-01T00:01:00Z".to_string()),
            size_bytes: Some(5_000_000),
            duration_ms: Some(60_000),
            success: true,
            error_message: None,
        };

        manager.log_download(&entry).unwrap();
    }

    #[test]
    fn test_active_model_operations() {
        let manager = ModelManager::open_in_memory().unwrap();

        // Initially empty
        let active = manager.get_active().unwrap();
        assert!(active.is_empty());

        // Insert an active record
        manager
            .insert_active(
                "server1",
                "model.gguf",
                "llama.cpp",
                1234,
                8080,
                "http://127.0.0.1:8080",
            )
            .unwrap();

        // Verify it appears
        let active = manager.get_active().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].server_name, "server1");
        assert_eq!(active[0].model_name, "model.gguf");
        assert_eq!(active[0].backend, "llama.cpp");
        assert_eq!(active[0].pid, 1234);
        assert_eq!(active[0].port, 8080);

        // Rename the active record
        manager.rename_active("server1", "server1-renamed").unwrap();
        let active = manager.get_active().unwrap();
        assert_eq!(active[0].server_name, "server1-renamed");

        // Remove the active record
        manager.remove_active("server1-renamed").unwrap();
        let active = manager.get_active().unwrap();
        assert!(active.is_empty());
    }

    #[test]
    fn test_download_queue_operations() {
        let manager = ModelManager::open_in_memory().unwrap();

        // Insert a queue item
        let id = manager
            .queue_insert(
                "pull-abc123",
                "owner/test-model",
                "test-model.Q4_K_M.gguf",
                Some("Test Model Q4"),
                "model",
                Some("Q4_K_M"),
                Some(4096),
            )
            .unwrap();
        assert!(id > 0);

        // Get queued item
        let item = manager.queue_get_queued().unwrap().unwrap();
        assert_eq!(item.job_id, "pull-abc123");
        assert_eq!(item.status, "queued");
        assert_eq!(item.kind, "model");
        assert_eq!(item.quant, Some("Q4_K_M".to_string()));
        assert_eq!(item.context_length, Some(4096));

        // Update status to running
        manager
            .queue_update_status("pull-abc123", "running", 500, Some(1000), None)
            .unwrap();

        // Get by job_id
        let item = manager.queue_get_by_job_id("pull-abc123").unwrap().unwrap();
        assert_eq!(item.status, "running");
        assert_eq!(item.bytes_downloaded, 500);

        // Get active items
        let active = manager.queue_get_active().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].job_id, "pull-abc123");

        // Complete the item
        manager
            .queue_update_status("pull-abc123", "completed", 1000, Some(1000), None)
            .unwrap();

        // Should appear in history now
        let history = manager.queue_get_history(10, 0).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, "completed");

        // Should no longer be in active
        let active = manager.queue_get_active().unwrap();
        assert!(active.is_empty());
    }

    #[test]
    fn test_queue_cancel() {
        let manager = ModelManager::open_in_memory().unwrap();

        manager
            .queue_insert(
                "pull-cancel1",
                "owner/test",
                "test.gguf",
                None,
                "model",
                None,
                None,
            )
            .unwrap();

        manager.queue_cancel("pull-cancel1").unwrap();

        let item = manager
            .queue_get_by_job_id("pull-cancel1")
            .unwrap()
            .unwrap();
        assert_eq!(item.status, "cancelled");
        assert!(item.completed_at.is_some());
    }

    #[test]
    fn test_update_check_operations() {
        let manager = ModelManager::open_in_memory().unwrap();

        // Initially no update check
        let check = manager.get_update_check("backend", "llama.cpp").unwrap();
        assert!(check.is_none());

        // Upsert an update check
        let params = UpdateCheckParams {
            item_type: "backend",
            item_id: "llama.cpp",
            current_version: Some("0.1"),
            latest_version: Some("0.2"),
            update_available: true,
            status: "update_available",
            error_message: None,
            details_json: None,
            checked_at: 1700000000,
        };
        manager.upsert_update_check(params).unwrap();

        // Retrieve it
        let check = manager
            .get_update_check("backend", "llama.cpp")
            .unwrap()
            .unwrap();
        assert_eq!(check.item_type, "backend");
        assert_eq!(check.item_id, "llama.cpp");
        assert_eq!(check.current_version, Some("0.1".to_string()));
        assert_eq!(check.latest_version, Some("0.2".to_string()));
        assert!(check.update_available);

        // Delete it
        manager.delete_update_check("backend", "llama.cpp").unwrap();
        let check = manager.get_update_check("backend", "llama.cpp").unwrap();
        assert!(check.is_none());
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
