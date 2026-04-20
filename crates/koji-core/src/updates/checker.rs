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

/// In-memory LRU cache for HuggingFace GGUF file listings.
/// Reduces API calls by caching (commit_sha, files) per repo_id for 5 minutes.
pub struct GgufListingCache {
    cache: std::sync::Arc<
        tokio::sync::Mutex<
            lru::LruCache<String, (String, Vec<crate::models::pull::RemoteGguf>, i64)>,
        >,
    >,
}

impl Clone for GgufListingCache {
    fn clone(&self) -> Self {
        Self {
            cache: std::sync::Arc::clone(&self.cache),
        }
    }
}

impl GgufListingCache {
    const TTL_SECS: i64 = 300; // 5 minutes
    const CAPACITY: usize = 64;

    pub fn new() -> Self {
        Self {
            cache: std::sync::Arc::new(tokio::sync::Mutex::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(Self::CAPACITY).unwrap(),
            ))),
        }
    }

    /// Get a cached entry if it exists and is fresh (within TTL).
    pub async fn get(
        &self,
        repo_id: &str,
    ) -> Option<(String, Vec<crate::models::pull::RemoteGguf>)> {
        let now = chrono::Utc::now().timestamp();
        let mut cache = self.cache.lock().await;
        if let Some(entry) = cache.get(repo_id) {
            let (sha, files, epoch) = entry;
            if now - *epoch < Self::TTL_SECS {
                return Some((sha.clone(), files.clone()));
            }
            // Stale — remove it so the next call fetches fresh data
            cache.pop(repo_id);
        }
        None
    }

    /// Store a result in the cache with the current timestamp.
    pub async fn insert(
        &self,
        repo_id: String,
        commit_sha: String,
        files: Vec<crate::models::pull::RemoteGguf>,
    ) {
        let now = chrono::Utc::now().timestamp();
        let mut cache = self.cache.lock().await;
        cache.put(repo_id, (commit_sha, files, now));
    }
}

/// Shared state for the update checker. Uses Arc<Mutex<()>> as a binary semaphore
/// to ensure that only one update check run occurs at any given time across the system.
/// Locking this guard serializes checks without needing to protect specific shared data.
#[derive(Clone)]
pub struct UpdateChecker {
    /// Mutex used as a synchronization primitive to prevent concurrent check runs.
    lock: Arc<Mutex<()>>,
    /// In-memory LRU cache for remote GGUF listings.
    gguf_listing_cache: GgufListingCache,
}

/// Results from an initial sync of backends and models to check for updates.
pub type UpdateSyncResults = (Vec<(String, BackendType)>, Vec<(i64, Option<String>)>);

impl UpdateChecker {
    pub fn new() -> Self {
        Self {
            lock: Arc::new(Mutex::new(())),
            gguf_listing_cache: GgufListingCache::new(),
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
        // Check cache before making network call to list_gguf_files
        let remote_listing = match self.gguf_listing_cache.get(repo_id).await {
            Some((cached_sha, cached_files)) => {
                tracing::debug!("GGUF listing cache hit for '{}'", repo_id);
                // Use cached file list — still need LFS hashes from fetch_blob_metadata
                let _blobs = pull::fetch_blob_metadata(repo_id).await?;
                crate::models::pull::RepoGgufListing {
                    repo_id: repo_id.to_string(),
                    commit_sha: cached_sha,
                    files: cached_files,
                }
            }
            None => pull::list_gguf_files(repo_id).await?,
        };

        // After successful fetch, store in cache (only if not already cached)
        if self.gguf_listing_cache.get(repo_id).await.is_none() {
            self.gguf_listing_cache
                .insert(
                    repo_id.to_string(),
                    remote_listing.commit_sha.clone(),
                    remote_listing.files.clone(),
                )
                .await;
        }

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

/// Determine the update status and availability based on file comparison results.
/// Returns (update_available, status, error_message).
pub fn determine_update_status(
    file_statuses: &[FileStatus],
) -> (bool, &'static str, Option<&'static str>) {
    let has_unknown = file_statuses
        .iter()
        .any(|s| matches!(s, FileStatus::Unknown));
    let has_changes = file_statuses.iter().any(|s| {
        matches!(
            s,
            FileStatus::Changed { .. } | FileStatus::NewRemote | FileStatus::RemovedFromRemote
        )
    });

    if has_unknown {
        (
            false,
            "verification_failed",
            Some("No stored hashes — run `model update --refresh`"),
        )
    } else if has_changes {
        (true, "update_available", None)
    } else {
        (false, "up_to_date", None)
    }
}

/// Check if enough time has passed since the last check based on interval.
pub fn should_check_since(
    oldest_check_timestamp: Option<i64>,
    interval_secs: i64,
    now: i64,
) -> bool {
    match oldest_check_timestamp {
        Some(ts) => now - ts >= interval_secs,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::update::FileStatus;

    // ── determine_update_status tests ─────────────────────────────────────

    #[test]
    fn test_determine_update_status_no_files() {
        let statuses: Vec<FileStatus> = vec![];
        let (available, status, _error) = determine_update_status(&statuses);
        assert!(!available);
        assert_eq!(status, "up_to_date");
    }

    #[test]
    fn test_determine_update_status_all_unchanged() {
        let statuses = vec![FileStatus::Unchanged, FileStatus::Unchanged];
        let (available, status, _error) = determine_update_status(&statuses);
        assert!(!available);
        assert_eq!(status, "up_to_date");
    }

    #[test]
    fn test_determine_update_status_has_changes() {
        let statuses = vec![
            FileStatus::Unchanged,
            FileStatus::Changed {
                old_oid: "abc".to_string(),
                new_oid: "def".to_string(),
            },
        ];
        let (available, status, _error) = determine_update_status(&statuses);
        assert!(available);
        assert_eq!(status, "update_available");
    }

    #[test]
    fn test_determine_update_status_new_remote() {
        let statuses = vec![FileStatus::Unchanged, FileStatus::NewRemote];
        let (available, status, _error) = determine_update_status(&statuses);
        assert!(available);
        assert_eq!(status, "update_available");
    }

    #[test]
    fn test_determine_update_status_unknown_hashes() {
        let statuses = vec![FileStatus::Unchanged, FileStatus::Unknown];
        let (available, status, error) = determine_update_status(&statuses);
        assert!(!available);
        assert_eq!(status, "verification_failed");
        assert!(error.is_some());
        assert!(error.unwrap().contains("No stored hashes"));
    }

    #[test]
    fn test_determine_update_status_unknown_overrides_changes() {
        // Unknown should take priority over changes
        let statuses = vec![
            FileStatus::Changed {
                old_oid: "a".to_string(),
                new_oid: "b".to_string(),
            },
            FileStatus::Unknown,
        ];
        let (available, status, _error) = determine_update_status(&statuses);
        assert!(!available);
        assert_eq!(status, "verification_failed");
    }

    #[test]
    fn test_determine_update_status_only_unknown() {
        let statuses = vec![FileStatus::Unknown];
        let (available, status, _error) = determine_update_status(&statuses);
        assert!(!available);
        assert_eq!(status, "verification_failed");
    }

    #[test]
    fn test_determine_update_status_removed_from_remote() {
        // RemovedFromRemote counts as a change
        let statuses = vec![FileStatus::RemovedFromRemote];
        let (available, status, _error) = determine_update_status(&statuses);
        assert!(available);
        assert_eq!(status, "update_available");
    }

    // ── should_check_since tests ──────────────────────────────────────────

    #[test]
    fn test_should_check_since_no_prior_check() {
        // No prior check → should always check
        assert!(should_check_since(None, 3600, 1000));
        assert!(should_check_since(None, 86400, 500));
    }

    #[test]
    fn test_should_check_since_interval_elapsed() {
        // Last check was 2 hours ago, interval is 1 hour → should check
        assert!(should_check_since(Some(0), 3600, 7200));
    }

    #[test]
    fn test_should_check_since_interval_not_elapsed() {
        // Last check was 30 minutes ago, interval is 1 hour → should not check
        assert!(!should_check_since(Some(0), 3600, 1800));
    }

    #[test]
    fn test_should_check_since_exact_boundary() {
        // Exactly at the boundary → should check (>=)
        assert!(should_check_since(Some(0), 3600, 3600));
    }

    #[test]
    fn test_should_check_since_one_second_over() {
        // One second over the interval → should check
        assert!(should_check_since(Some(0), 3600, 3601));
    }

    #[test]
    fn test_should_check_since_large_interval() {
        // 24-hour interval, checked 23h ago → should not check
        assert!(!should_check_since(Some(0), 86400, 82800));
        // 24-hour interval, checked 25h ago → should check
        assert!(should_check_since(Some(0), 86400, 90000));
    }

    #[test]
    fn test_should_check_since_zero_interval() {
        // Zero interval means always check (even with prior check)
        assert!(should_check_since(Some(1000), 0, 1000));
        assert!(should_check_since(Some(1000), 0, 2000));
    }

    // ── UpdateChecker construction tests ──────────────────────────────────

    #[test]
    fn test_update_checker_new() {
        let checker = UpdateChecker::new();
        // Just verify it constructs without panicking
        let _ = checker.clone();
    }

    #[test]
    fn test_update_checker_default() {
        let checker = UpdateChecker::default();
        let _ = checker.clone();
    }

    #[test]
    fn test_update_checker_clone() {
        let checker1 = UpdateChecker::new();
        let checker2 = checker1.clone();
        // Both should be usable independently
        let _ = checker1;
        let _ = checker2;
    }

    // ── GgufListingCache tests ────────────────────────────────────────────

    #[test]
    fn test_gguf_listing_cache_new() {
        let cache = GgufListingCache::new();
        // Just verify it constructs without panicking
        let _ = cache;
    }

    #[test]
    fn test_gguf_listing_cache_clone() {
        let cache1 = GgufListingCache::new();
        let cache2 = cache1.clone();
        // Both should be usable independently
        let _ = cache1;
        let _ = cache2;
    }
}
