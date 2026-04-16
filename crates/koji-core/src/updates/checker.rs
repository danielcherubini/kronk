use std::sync::Arc;
use tokio::sync::Mutex;

use crate::backends::{check_latest_version, BackendRegistry, BackendType};
use crate::config::Config;
use crate::db;
use crate::db::queries::{
    get_active_backend, get_all_update_checks, get_model_pull, get_oldest_check_time,
};
use crate::models::pull::list_gguf_files;

/// Shared state for the update checker. Uses Arc<Mutex<()>> as a binary semaphore
/// to ensure that only one update check run occurs at any given time across the system.
/// Locking this guard serializes checks without needing to protect specific shared data.
#[derive(Clone)]
pub struct UpdateChecker {
    /// Mutex used as a synchronization primitive to prevent concurrent check runs.
    lock: Arc<Mutex<()>>,
}

/// Results from an initial sync of backends and models to check for updates.
pub type UpdateSyncResults = (Vec<(String, BackendType)>, Vec<(String, Option<String>)>);

impl UpdateChecker {
    pub fn new() -> Self {
        Self {
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Run a full update check for all backends and models.
    /// Returns immediately if another check is already in progress.
    pub async fn run_check(&self, config_dir: &std::path::Path) -> anyhow::Result<()> {
        // Try to acquire the lock
        let _guard = match self.lock.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::info!("Update check already in progress, skipping");
                return Ok(());
            }
        };

        tracing::info!("Starting update check for all items");

        // Phase 1: Sync DB - fetch all items to check
        let (backends, models) = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            move || -> anyhow::Result<UpdateSyncResults> {
                let registry = BackendRegistry::open(&config_dir)?;
                let backends: Vec<(String, BackendType)> = registry
                    .list()
                    .unwrap_or_default()
                    .iter()
                    .map(|b| (b.name.clone(), b.backend_type.clone()))
                    .collect();

                let open = db::open(&config_dir)?;
                let db_models = db::load_model_configs(&open.conn)?;
                let models: Vec<(String, Option<String>)> = db_models
                    .into_iter()
                    .map(|(key, mc)| (key, mc.model.clone()))
                    .collect();

                Ok((backends, models))
            }
        })
        .await??;

        // Phase 2: Async network - check each backend
        for (backend_name, backend_type) in &backends {
            if let Err(e) = self
                .check_backend(config_dir, backend_name, backend_type)
                .await
            {
                tracing::warn!("Failed to check backend {}: {}", backend_name, e);
            }
        }

        // Phase 2: Async network - check each model
        for (model_id, repo_id) in &models {
            if let Err(e) = self
                .check_model(config_dir, model_id, repo_id.as_deref())
                .await
            {
                tracing::warn!("Failed to check model {}: {}", model_id, e);
            }
        }

        tracing::info!("Update check complete");
        Ok(())
    }

    /// Check a single backend for updates.
    pub async fn check_backend(
        &self,
        config_dir: &std::path::Path,
        backend_name: &str,
        backend_type: &BackendType,
    ) -> anyhow::Result<()> {
        // Sync: Get current version from DB
        let current_version = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let backend_name = backend_name.to_string();
            move || -> anyhow::Result<Option<String>> {
                let open = db::open(&config_dir)?;
                let record = get_active_backend(&open.conn, &backend_name)?;
                Ok(record.map(|r| r.version))
            }
        })
        .await??;

        // Async: Check latest version from network
        let latest_version = match backend_type {
            BackendType::LlamaCpp | BackendType::IkLlama => {
                match check_latest_version(backend_type).await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        self.save_check_result(
                            config_dir,
                            "backend",
                            backend_name,
                            current_version.as_deref(),
                            None,
                            false,
                            "error",
                            Some(&e.to_string()),
                            None,
                        )
                        .await?;
                        return Ok(());
                    }
                }
            }
            BackendType::Custom => None,
        };

        let update_available = latest_version
            .as_ref()
            .map(|v| current_version.as_ref().map(|c| v != c).unwrap_or(true))
            .unwrap_or(false);

        let status = if latest_version.is_none() && current_version.is_none() {
            "unknown"
        } else if update_available {
            "update_available"
        } else {
            "up_to_date"
        };

        self.save_check_result(
            config_dir,
            "backend",
            backend_name,
            current_version.as_deref(),
            latest_version.as_deref(),
            update_available,
            status,
            None,
            None,
        )
        .await
    }

    /// Check a single model for updates.
    /// Uses 3-phase approach: sync DB read, async HF network, sync DB write.
    pub async fn check_model(
        &self,
        config_dir: &std::path::Path,
        model_id: &str,
        repo_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let repo_id = match repo_id.filter(|s| !s.is_empty()) {
            Some(id) => id,
            None => {
                self.save_check_result(
                    config_dir,
                    "model",
                    model_id,
                    None,
                    None,
                    false,
                    "unknown",
                    Some("Model has no source repo configured"),
                    None,
                )
                .await?;
                return Ok(());
            }
        };

        // Sync: Get current commit from DB
        let current_version = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let repo_id = repo_id.to_string();
            move || -> anyhow::Result<Option<String>> {
                let open = db::open(&config_dir)?;
                let pull = get_model_pull(&open.conn, &repo_id)?;
                Ok(pull.map(|p| p.commit_sha))
            }
        })
        .await??;

        // Async: List remote files
        let latest_listing = match list_gguf_files(repo_id).await {
            Ok(l) => l,
            Err(e) => {
                self.save_check_result(
                    config_dir,
                    "model",
                    model_id,
                    current_version.as_deref(),
                    None,
                    false,
                    "error",
                    Some(&e.to_string()),
                    None,
                )
                .await?;
                return Ok(());
            }
        };

        // Pure logic - determine update availability
        let update_available = current_version
            .as_ref()
            .map(|c| *c != latest_listing.commit_sha)
            .unwrap_or(true);

        let details_json = serde_json::json!({
            "repo_id": latest_listing.repo_id,
            "commit_sha": latest_listing.commit_sha,
            "file_count": latest_listing.files.len(),
            "files": latest_listing.files.iter().map(|f| f.filename.clone()).collect::<Vec<_>>(),
        })
        .to_string();

        let status = if update_available {
            "update_available"
        } else {
            "up_to_date"
        };

        self.save_check_result(
            config_dir,
            "model",
            model_id,
            current_version.as_deref(),
            Some(&latest_listing.commit_sha),
            update_available,
            status,
            None,
            Some(&details_json),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn save_check_result(
        &self,
        config_dir: &std::path::Path,
        item_type: &str,
        item_id: &str,
        current_version: Option<&str>,
        latest_version: Option<&str>,
        update_available: bool,
        status: &str,
        error_message: Option<&str>,
        details_json: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        let status_str = status.to_string();
        tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let item_type = item_type.to_string();
            let item_id = item_id.to_string();
            let current_version = current_version.map(String::from);
            let latest_version = latest_version.map(String::from);
            let error_message = error_message.map(String::from);
            let details_json = details_json.map(String::from);
            let status = status_str;
            move || -> anyhow::Result<()> {
                let open = db::open(&config_dir)?;
                crate::db::queries::upsert_update_check(
                    &open.conn,
                    crate::db::queries::UpdateCheckParams {
                        item_type: &item_type,
                        item_id: &item_id,
                        current_version: current_version.as_deref(),
                        latest_version: latest_version.as_deref(),
                        update_available,
                        status: &status,
                        error_message: error_message.as_deref(),
                        details_json: details_json.as_deref(),
                        checked_at: now,
                    },
                )?;
                Ok(())
            }
        })
        .await??;
        Ok(())
    }

    /// Get cached update check results.
    pub async fn get_results(
        &self,
        config_dir: &std::path::Path,
    ) -> anyhow::Result<Vec<crate::db::queries::UpdateCheckRecord>> {
        tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            move || -> anyhow::Result<Vec<crate::db::queries::UpdateCheckRecord>> {
                let open = db::open(&config_dir)?;
                get_all_update_checks(&open.conn)
            }
        })
        .await?
    }

    /// Check if enough time has passed since last check (based on interval).
    pub async fn should_check(&self, config_dir: &std::path::Path) -> anyhow::Result<bool> {
        let config_dir_for_config = config_dir.to_path_buf();
        let config = tokio::task::spawn_blocking(move || Config::load_from(&config_dir_for_config))
            .await??;

        let interval_hours = config.general.update_check_interval as i64;
        let interval_secs = interval_hours * 3600;

        let oldest = tokio::task::spawn_blocking({
            let config_dir_for_db = config_dir.to_path_buf();
            move || -> anyhow::Result<Option<i64>> {
                let open = db::open(&config_dir_for_db)?;
                get_oldest_check_time(&open.conn)
            }
        })
        .await??;

        let now = chrono::Utc::now().timestamp();
        match oldest {
            Some(ts) => Ok(now - ts >= interval_secs),
            None => Ok(true),
        }
    }
}

impl Default for UpdateChecker {
    fn default() -> Self {
        Self::new()
    }
}
