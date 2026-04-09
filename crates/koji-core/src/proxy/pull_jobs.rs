use serde::Serialize;
use std::time::Instant;

/// Status of a pull job.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PullJobStatus {
    Pending,
    Running,
    /// Download finished, now hashing the file and comparing to the HF LFS SHA-256.
    Verifying,
    Completed,
    Failed,
}

/// A pull job for downloading a model from HuggingFace.
#[derive(Debug, Clone, Serialize)]
pub struct PullJob {
    pub job_id: String,
    pub repo_id: String,
    pub filename: String,
    pub status: PullJobStatus,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
    /// Bytes hashed during the verify phase. Updated by the progress-poll task.
    #[serde(default)]
    pub verify_bytes_hashed: u64,
    /// Total bytes to hash during the verify phase (== downloaded file size).
    /// Set when the verify phase starts so the client can render a progress bar.
    #[serde(default)]
    pub verify_total_bytes: Option<u64>,
    /// Verification outcome. `None` when verification has not completed or when
    /// no upstream LFS hash was available (can't verify). `Some(true)` on match,
    /// `Some(false)` on mismatch or hashing error.
    #[serde(default)]
    pub verified_ok: Option<bool>,
    /// Human-readable verification error detail. Mutually set with `verified_ok`.
    #[serde(default)]
    pub verify_error: Option<String>,
    /// Set when status transitions to Completed or Failed; used for eviction.
    #[serde(skip)]
    pub completed_at: Option<Instant>,
}

impl Default for PullJob {
    fn default() -> Self {
        Self {
            job_id: String::new(),
            repo_id: String::new(),
            filename: String::new(),
            status: PullJobStatus::Pending,
            bytes_downloaded: 0,
            total_bytes: None,
            error: None,
            verify_bytes_hashed: 0,
            verify_total_bytes: None,
            verified_ok: None,
            verify_error: None,
            completed_at: None,
        }
    }
}
