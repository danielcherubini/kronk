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

    // ── Async wrappers ────────────────────────────────────────

    /// Check for HuggingFace updates for a model. Async wrapper around
    /// `crate::models::update::check_for_updates`.
    ///
    /// Note: this method is `!Send` because `Connection: !Send`.
    /// Callers must use `tokio::task::spawn_blocking` or similar.
    pub async fn check_for_updates(
        &self,
        repo_id: &str,
    ) -> Result<crate::models::update::UpdateCheckResult> {
        crate::models::update::check_for_updates(&self.conn, repo_id).await
    }

    /// Refresh HuggingFace metadata for a model. Async wrapper around
    /// `crate::models::update::refresh_metadata`.
    ///
    /// Note: this method is `!Send` because `Connection: !Send`.
    pub async fn refresh_metadata(&self, models_dir: &Path, repo_id: &str) -> Result<()> {
        crate::models::update::refresh_metadata(&self.conn, models_dir, repo_id).await
    }
}
