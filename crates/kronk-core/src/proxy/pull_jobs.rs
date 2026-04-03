use serde::Serialize;
use std::time::Instant;

/// Status of a pull job.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PullJobStatus {
    Pending,
    Running,
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
            completed_at: None,
        }
    }
}
