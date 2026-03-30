//! Core logic for detecting model updates from HuggingFace.
//!
//! The strategy is two-tier:
//! 1. Quick check — compare stored `commit_sha` against remote `RepoInfo.sha`.
//!    If identical, the model is up-to-date (single lightweight API call).
//! 2. Per-file check — if the commit SHA differs, fetch blob metadata to compare
//!    individual file LFS SHA256 hashes. This avoids re-downloading when only
//!    non-GGUF files changed (e.g., README updates).
//!
//! ## Design constraint
//! `rusqlite::Connection` is `!Send`. All functions that touch both DB and network
//! are structured as: sync DB reads → async network → sync DB writes. The
//! `&Connection` is never referenced across `.await` points.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

use crate::db::queries::{
    get_model_files, get_model_pull, upsert_model_file, upsert_model_pull, ModelFileRecord,
};
use crate::models::pull::{self, BlobInfo};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of checking a single model for updates.
#[derive(Debug)]
pub struct UpdateCheckResult {
    pub repo_id: String,
    pub status: UpdateStatus,
    pub file_updates: Vec<FileUpdateInfo>,
}

/// High-level update status for a model.
#[derive(Debug)]
pub enum UpdateStatus {
    /// No stored metadata — model was pulled before DB existed
    NoPriorRecord,
    /// Commit SHA matches — repo hasn't changed at all
    UpToDate,
    /// Commit SHA differs but all tracked GGUF files are unchanged
    RepoChangedFilesUnchanged,
    /// One or more tracked GGUF files have changed
    UpdatesAvailable,
    /// Error checking (network, API, etc.)
    CheckFailed(String),
}

/// Per-file update status.
#[derive(Debug)]
pub struct FileUpdateInfo {
    pub filename: String,
    pub quant: Option<String>,
    pub status: FileStatus,
    pub local_size: Option<i64>,
    pub remote_size: Option<i64>,
}

/// Status of an individual file compared to remote.
#[derive(Debug)]
pub enum FileStatus {
    Unchanged,
    /// LFS SHA256 changed
    Changed {
        old_oid: String,
        new_oid: String,
    },
    /// New remote file not locally downloaded
    NewRemote,
    /// No stored hash to compare (legacy pull without DB)
    Unknown,
    /// File was removed from remote
    RemovedFromRemote,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Check a single model for updates against HuggingFace.
///
/// Network errors are captured as `UpdateStatus::CheckFailed` so that one
/// model's failure doesn't abort checking all others. DB errors propagate as `Err`.
///
/// Structured to avoid holding `&Connection` across `.await` points:
/// all DB reads happen before the first `.await`, all DB writes happen after.
pub async fn check_for_updates(conn: &Connection, repo_id: &str) -> Result<UpdateCheckResult> {
    // Step 1: SYNC — read from DB (no .await)
    let pull_record = get_model_pull(conn, repo_id)?;
    let file_records = get_model_files(conn, repo_id)?;

    // Step 2: handle no prior record
    if pull_record.is_none() {
        return Ok(UpdateCheckResult {
            repo_id: repo_id.to_string(),
            status: UpdateStatus::NoPriorRecord,
            file_updates: vec![],
        });
    }
    let pull_record = pull_record.unwrap();

    // Step 3: ASYNC — fetch remote state (conn not referenced after this point)
    let remote_listing = match pull::list_gguf_files(repo_id).await {
        Ok(listing) => listing,
        Err(e) => {
            return Ok(UpdateCheckResult {
                repo_id: repo_id.to_string(),
                status: UpdateStatus::CheckFailed(e.to_string()),
                file_updates: vec![],
            });
        }
    };

    // Step 4: quick check — commit SHA match?
    if remote_listing.commit_sha == pull_record.commit_sha {
        return Ok(UpdateCheckResult {
            repo_id: repo_id.to_string(),
            status: UpdateStatus::UpToDate,
            file_updates: vec![],
        });
    }

    // Step 5: ASYNC — fetch per-file blob metadata
    let remote_blobs = match pull::fetch_blob_metadata(repo_id).await {
        Ok(blobs) => blobs,
        Err(e) => {
            return Ok(UpdateCheckResult {
                repo_id: repo_id.to_string(),
                status: UpdateStatus::CheckFailed(format!(
                    "Commit changed but failed to fetch file details: {}",
                    e
                )),
                file_updates: vec![],
            });
        }
    };

    // Step 6: PURE — compare local vs remote (testable, no I/O)
    let file_updates = compare_files(&file_records, &remote_blobs);

    // Step 7: determine overall status
    let has_changes = file_updates
        .iter()
        .any(|f| matches!(f.status, FileStatus::Changed { .. } | FileStatus::NewRemote));

    let status = if has_changes {
        UpdateStatus::UpdatesAvailable
    } else {
        UpdateStatus::RepoChangedFilesUnchanged
    };

    Ok(UpdateCheckResult {
        repo_id: repo_id.to_string(),
        status,
        file_updates,
    })
}

/// Compare local file records against remote blob metadata.
///
/// This is a pure function with no I/O — fully unit-testable.
pub fn compare_files(
    local_files: &[ModelFileRecord],
    remote_blobs: &HashMap<String, BlobInfo>,
) -> Vec<FileUpdateInfo> {
    let mut results: Vec<FileUpdateInfo> = Vec::new();

    // Check all local files against remote
    for local in local_files {
        if let Some(remote) = remote_blobs.get(&local.filename) {
            let status = match (&local.lfs_oid, &remote.lfs_sha256) {
                (Some(local_oid), Some(remote_oid)) => {
                    if local_oid == remote_oid {
                        FileStatus::Unchanged
                    } else {
                        FileStatus::Changed {
                            old_oid: local_oid.clone(),
                            new_oid: remote_oid.clone(),
                        }
                    }
                }
                _ => FileStatus::Unknown,
            };
            results.push(FileUpdateInfo {
                filename: local.filename.clone(),
                quant: local.quant.clone(),
                status,
                local_size: local.size_bytes,
                remote_size: remote.size,
            });
        } else {
            // Local file no longer in remote
            results.push(FileUpdateInfo {
                filename: local.filename.clone(),
                quant: local.quant.clone(),
                status: FileStatus::RemovedFromRemote,
                local_size: local.size_bytes,
                remote_size: None,
            });
        }
    }

    // Check for new remote files not in local
    for (filename, remote) in remote_blobs {
        let already_tracked = local_files.iter().any(|f| &f.filename == filename);
        if !already_tracked {
            results.push(FileUpdateInfo {
                filename: filename.clone(),
                quant: None,
                status: FileStatus::NewRemote,
                local_size: None,
                remote_size: remote.size,
            });
        }
    }

    results
}

/// Refresh DB metadata for a model without re-downloading.
///
/// Fetches current commit SHA and file LFS OIDs from HuggingFace and writes to DB.
/// Used to establish a baseline for models pulled before the DB existed.
///
/// Pattern: async fetches first → sync DB writes (avoids `!Send` issues).
pub async fn refresh_metadata(conn: &Connection, repo_id: &str) -> Result<()> {
    // ASYNC — fetch remote data
    let listing = pull::list_gguf_files(repo_id).await?;
    let blobs = pull::fetch_blob_metadata(repo_id).await?;

    // SYNC — write to DB
    upsert_model_pull(conn, repo_id, &listing.commit_sha)?;
    for file in &listing.files {
        let blob = blobs.get(&file.filename);
        upsert_model_file(
            conn,
            repo_id,
            &file.filename,
            file.quant.as_deref(),
            blob.and_then(|b| b.lfs_sha256.as_deref()),
            blob.and_then(|b| b.size),
        )?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, queries};

    fn make_file_record(
        filename: &str,
        lfs_oid: Option<&str>,
        size: Option<i64>,
    ) -> ModelFileRecord {
        ModelFileRecord {
            repo_id: "test/repo".to_string(),
            filename: filename.to_string(),
            quant: None,
            lfs_oid: lfs_oid.map(|s| s.to_string()),
            size_bytes: size,
            downloaded_at: "2024-01-01T00:00:00.000Z".to_string(),
        }
    }

    fn make_blob(filename: &str, sha256: Option<&str>, size: Option<i64>) -> BlobInfo {
        BlobInfo {
            filename: filename.to_string(),
            blob_id: None,
            size,
            lfs_sha256: sha256.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_compare_files_unchanged() {
        let local = vec![make_file_record("model.gguf", Some("sha_abc"), Some(1000))];
        let mut remote = HashMap::new();
        remote.insert(
            "model.gguf".to_string(),
            make_blob("model.gguf", Some("sha_abc"), Some(1000)),
        );

        let result = compare_files(&local, &remote);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].status, FileStatus::Unchanged));
    }

    #[test]
    fn test_compare_files_changed() {
        let local = vec![make_file_record("model.gguf", Some("sha_old"), Some(1000))];
        let mut remote = HashMap::new();
        remote.insert(
            "model.gguf".to_string(),
            make_blob("model.gguf", Some("sha_new"), Some(1100)),
        );

        let result = compare_files(&local, &remote);
        assert_eq!(result.len(), 1);
        match &result[0].status {
            FileStatus::Changed { old_oid, new_oid } => {
                assert_eq!(old_oid, "sha_old");
                assert_eq!(new_oid, "sha_new");
            }
            other => panic!("expected Changed, got {:?}", other),
        }
        assert_eq!(result[0].local_size, Some(1000));
        assert_eq!(result[0].remote_size, Some(1100));
    }

    #[test]
    fn test_compare_files_new_remote() {
        let local: Vec<ModelFileRecord> = vec![];
        let mut remote = HashMap::new();
        remote.insert(
            "model-new.gguf".to_string(),
            make_blob("model-new.gguf", Some("sha_new"), Some(2000)),
        );

        let result = compare_files(&local, &remote);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].status, FileStatus::NewRemote));
        assert_eq!(result[0].filename, "model-new.gguf");
    }

    #[test]
    fn test_compare_files_removed() {
        let local = vec![make_file_record(
            "old-model.gguf",
            Some("sha_abc"),
            Some(1000),
        )];
        let remote: HashMap<String, BlobInfo> = HashMap::new();

        let result = compare_files(&local, &remote);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].status, FileStatus::RemovedFromRemote));
    }

    #[test]
    fn test_compare_files_unknown() {
        // Local file has no lfs_oid (legacy pull)
        let local = vec![make_file_record("model.gguf", None, Some(1000))];
        let mut remote = HashMap::new();
        remote.insert(
            "model.gguf".to_string(),
            make_blob("model.gguf", Some("sha_abc"), Some(1000)),
        );

        let result = compare_files(&local, &remote);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].status, FileStatus::Unknown));
    }

    #[test]
    fn test_compare_files_mixed() {
        let local = vec![
            make_file_record("unchanged.gguf", Some("sha_same"), Some(1000)),
            make_file_record("changed.gguf", Some("sha_old"), Some(2000)),
            make_file_record("removed.gguf", Some("sha_gone"), Some(3000)),
            make_file_record("no_hash.gguf", None, Some(4000)),
        ];
        let mut remote = HashMap::new();
        remote.insert(
            "unchanged.gguf".to_string(),
            make_blob("unchanged.gguf", Some("sha_same"), Some(1000)),
        );
        remote.insert(
            "changed.gguf".to_string(),
            make_blob("changed.gguf", Some("sha_new"), Some(2100)),
        );
        remote.insert(
            "new.gguf".to_string(),
            make_blob("new.gguf", Some("sha_fresh"), Some(5000)),
        );
        remote.insert(
            "no_hash.gguf".to_string(),
            make_blob("no_hash.gguf", Some("sha_abc"), Some(4000)),
        );

        let result = compare_files(&local, &remote);
        // 4 local + 1 new remote = 5
        assert_eq!(result.len(), 5);

        let unchanged = result
            .iter()
            .find(|f| f.filename == "unchanged.gguf")
            .unwrap();
        assert!(matches!(unchanged.status, FileStatus::Unchanged));

        let changed = result
            .iter()
            .find(|f| f.filename == "changed.gguf")
            .unwrap();
        assert!(matches!(changed.status, FileStatus::Changed { .. }));

        let removed = result
            .iter()
            .find(|f| f.filename == "removed.gguf")
            .unwrap();
        assert!(matches!(removed.status, FileStatus::RemovedFromRemote));

        let new = result.iter().find(|f| f.filename == "new.gguf").unwrap();
        assert!(matches!(new.status, FileStatus::NewRemote));

        let unknown = result
            .iter()
            .find(|f| f.filename == "no_hash.gguf")
            .unwrap();
        assert!(matches!(unknown.status, FileStatus::Unknown));
    }

    #[test]
    fn test_check_no_prior_record() {
        // Uses in-memory DB with no data — should return NoPriorRecord
        let conn = open_in_memory().unwrap();
        // We can't call the async function directly in a sync test,
        // so we test the sync DB path by checking the query directly.
        let result = queries::get_model_pull(&conn, "test/repo").unwrap();
        assert!(result.is_none(), "expected no prior record in empty DB");
    }
}
