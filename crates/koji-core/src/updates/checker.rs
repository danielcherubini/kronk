use std::sync::Arc;
use tokio::sync::Mutex;

use crate::backends::{check_latest_version, BackendRegistry, BackendType};
use crate::config::Config;
use crate::db;
use crate::db::queries::{
    get_active_backend, get_all_model_configs, get_all_update_checks, get_model_pull,
    get_oldest_check_time,
};
use crate::models::{
    pull,
    update::{compare_files, FileStatus},
};

/// Shared state for the update checker. Uses Arc<Mutex<()>> as a binary semaphore
/// to ensure that only one update check run occurs at any given time across the system.
/// Locking this guard serializes checks without needing to protect specific shared data.
#[derive(Clone)]
pub struct UpdateChecker {
    /// Mutex used as a synchronization primitive to prevent concurrent check runs.
    lock: Arc<Mutex<()>>,
}

/// Results from an initial sync of backends and models to check for updates.
pub type UpdateSyncResults = (Vec<(String, BackendType)>, Vec<(i64, Option<String>)>);

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
                let db_model_records = get_all_model_configs(&open.conn)?;
                let models: Vec<(i64, Option<String>)> = db_model_records
                    .into_iter()
                    .map(|r| (r.id, Some(r.repo_id)))
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
                .check_model(config_dir, *model_id, repo_id.as_deref())
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
    /// Uses the same two-tier strategy as `models::update::check_for_updates`:
    /// (1) commit SHA quick check, then (2) per-file LFS hash comparison so
    /// that non-GGUF repo changes don't trigger false positives.
    pub async fn check_model(
        &self,
        config_dir: &std::path::Path,
        model_id: i64,
        repo_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let repo_id = match repo_id {
            Some(id) if !id.is_empty() => id,
            _ => {
                self.save_check_result(
                    config_dir,
                    "model",
                    &model_id.to_string(),
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

        // Phase 1 — SYNC: read DB state (no .await)
        let db_state = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let repo_id = repo_id.to_string();
            move || -> anyhow::Result<Option<(db::queries::ModelPullRecord, Vec<db::queries::ModelFileRecord>)>> {
                let open = db::open(&config_dir)?;
                let model_record =
                    match db::queries::get_model_config_by_repo_id(&open.conn, &repo_id)? {
                        Some(r) => r,
                        None => return Ok(None),
                    };
                let pull_record = get_model_pull(&open.conn, model_record.id)?;
                let file_records = db::queries::get_model_files(&open.conn, model_record.id)?;
                Ok(pull_record.map(|pr| (pr, file_records)))
            }
        })
        .await??;

        // Handle no prior record
        let Some((pull_record, file_records)) = db_state else {
            self.save_check_result(
                config_dir,
                "model",
                &model_id.to_string(),
                None,
                None,
                false,
                "no_prior_record",
                None,
                None,
            )
            .await?;
            return Ok(());
        };

        // Phase 2 — ASYNC: fetch remote state (conn not referenced after this point)
        let remote_listing = match pull::list_gguf_files(repo_id).await {
            Ok(l) => l,
            Err(e) => {
                self.save_check_result(
                    config_dir,
                    "model",
                    &model_id.to_string(),
                    Some(&pull_record.commit_sha),
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

        // Tier 1 — quick check: commit SHA match?
        if remote_listing.commit_sha == pull_record.commit_sha {
            self.save_check_result(
                config_dir,
                "model",
                &model_id.to_string(),
                Some(&pull_record.commit_sha),
                Some(&remote_listing.commit_sha),
                false,
                "up_to_date",
                None,
                None,
            )
            .await?;
            return Ok(());
        }

        // Tier 2 — per-file LFS hash comparison
        let resolved_repo_id = &remote_listing.repo_id;
        let remote_blobs = match pull::fetch_blob_metadata(resolved_repo_id).await {
            Ok(blobs) => blobs,
            Err(e) => {
                self.save_check_result(
                    config_dir,
                    "model",
                    &model_id.to_string(),
                    Some(&pull_record.commit_sha),
                    Some(&remote_listing.commit_sha),
                    false,
                    "error",
                    Some(&format!(
                        "Commit changed but failed to fetch file details: {e}"
                    )),
                    None,
                )
                .await?;
                return Ok(());
            }
        };

        // Phase 3 — PURE: compare local vs remote (testable, no I/O)
        let file_updates = compare_files(&file_records, &remote_blobs);

        let has_unknown = file_updates
            .iter()
            .any(|f| matches!(f.status, FileStatus::Unknown));

        let has_changes = file_updates
            .iter()
            .any(|f| matches!(f.status, FileStatus::Changed { .. } | FileStatus::NewRemote));

        let (update_available, status, error_message) = if has_unknown {
            (
                false,
                "verification_failed",
                Some("No stored hashes — run `model update --refresh`"),
            )
        } else if has_changes {
            (true, "update_available", None)
        } else {
            (false, "up_to_date", None)
        };

        let details_json = serde_json::json!({
            "repo_id": remote_listing.repo_id,
            "commit_sha": remote_listing.commit_sha,
            "file_count": file_updates.len(),
            "files": file_updates.iter().map(|f| {
                serde_json::json!({
                    "filename": f.filename,
                    "status": format!("{:?}", f.status),
                    "quant": f.quant,
                })
            }).collect::<Vec<_>>(),
        })
        .to_string();

        self.save_check_result(
            config_dir,
            "model",
            &model_id.to_string(),
            Some(&pull_record.commit_sha),
            Some(&remote_listing.commit_sha),
            update_available,
            status,
            error_message,
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
