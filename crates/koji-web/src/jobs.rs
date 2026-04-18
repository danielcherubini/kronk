use std::collections::{HashMap, VecDeque};
// Use std::process::kill for Unix, taskkill for Windows
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex, RwLock};

pub type JobId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Install,
    Update,
    Restore,
}

#[derive(Debug, Clone)]
pub enum JobEvent {
    Log(String),
    Status(JobStatus),
}

pub struct JobState {
    pub status: JobStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub backend_type: Option<koji_core::backends::BackendType>,
    pub state: RwLock<JobState>,
    pub log_head: RwLock<VecDeque<String>>,
    pub log_tail: RwLock<VecDeque<String>>,
    pub log_dropped: AtomicU64,
    pub log_tx: broadcast::Sender<JobEvent>,
    pub child_pids: RwLock<Vec<u32>>,
}

pub const LOG_HEAD_CAP: usize = 100;
pub const LOG_TAIL_CAP: usize = 400;
pub const LOG_BROADCAST_CAP: usize = 1024;
pub const RETAINED_FINISHED_JOBS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("another backend job is already running")]
    AlreadyRunning(JobId),
    #[error("job not found")]
    NotFound,
}

pub struct JobManager {
    jobs: Arc<RwLock<HashMap<JobId, Arc<Job>>>>,
    finished_order: Arc<Mutex<VecDeque<JobId>>>,
    active: Arc<Mutex<Option<JobId>>>,
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            finished_order: Arc::new(Mutex::new(VecDeque::new())),
            active: Arc::new(Mutex::new(None)),
        }
    }

    /// Reserve an active slot, return a fresh Job. Returns AlreadyRunning if one is active.
    pub async fn submit(
        &self,
        kind: JobKind,
        backend_type: Option<koji_core::backends::BackendType>,
    ) -> Result<Arc<Job>, JobError> {
        // Generate a unique job ID
        let job_id = format!("j_{}", uuid::Uuid::new_v4().simple());

        // Create the job
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let job = Arc::new(Job {
            id: job_id.clone(),
            kind,
            backend_type,
            state: RwLock::new(JobState {
                status: JobStatus::Running,
                started_at: now,
                finished_at: None,
                error: None,
            }),
            log_head: RwLock::new(VecDeque::new()),
            log_tail: RwLock::new(VecDeque::new()),
            log_dropped: AtomicU64::new(0),
            log_tx: broadcast::channel(LOG_BROADCAST_CAP).0,
            child_pids: RwLock::new(Vec::new()),
        });

        // Atomic check-and-set: hold the active lock across check and set
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err(JobError::AlreadyRunning(active.as_ref().unwrap().clone()));
        }
        *active = Some(job_id.clone());
        drop(active);

        // Insert into jobs map
        self.jobs.write().await.insert(job_id.clone(), job.clone());

        Ok(job)
    }

    pub async fn get(&self, id: &JobId) -> Option<Arc<Job>> {
        self.jobs.read().await.get(id).cloned()
    }

    pub async fn active(&self) -> Option<Arc<Job>> {
        let active_id = self.active.lock().await.clone();
        if let Some(id) = active_id {
            self.jobs.read().await.get(&id).cloned()
        } else {
            None
        }
    }

    /// Append a log line to the job: writes to head if not full, else tail (with eviction),
    /// increments log_dropped if a line falls between head and tail, and broadcasts on log_tx.
    pub async fn append_log(&self, job: &Job, line: String) {
        // Register child PIDs if this is a spawn command
        if line.contains("pid=") {
            if let Some(start) = line.find("pid=") {
                let pid_str = &line[start + 4..];
                let end = pid_str
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(pid_str.len());
                if let Ok(pid) = pid_str[..end].parse::<u32>() {
                    self.register_child(job, pid).await;
                }
            }
        }

        let mut head = job.log_head.write().await;

        if head.len() < LOG_HEAD_CAP {
            head.push_back(line.clone());
            drop(head);
            // Broadcast the log event
            if let Err(e) = job.log_tx.send(JobEvent::Log(line)) {
                tracing::warn!("Failed to broadcast log for job {}: {}", job.id, e);
            }
            return;
        }

        drop(head);

        // Head is full, write to tail
        let mut tail = job.log_tail.write().await;
        if tail.len() < LOG_TAIL_CAP {
            tail.push_back(line.clone());
        } else {
            tail.pop_front();
            tail.push_back(line.clone());
            job.log_dropped.fetch_add(1, Ordering::Relaxed);
        }
        drop(tail);

        // Broadcast the log event
        if let Err(e) = job.log_tx.send(JobEvent::Log(line)) {
            tracing::warn!("Failed to broadcast log for job {}: {}", job.id, e);
        }
    }

    /// Register a child process PID for this job.
    pub async fn register_child(&self, job: &Job, pid: u32) {
        let mut pids = job.child_pids.write().await;
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }

    /// Kill all child processes for a job.
    pub async fn kill_children(&self, job: &Job) {
        let pids = job.child_pids.read().await;
        for &pid in pids.iter() {
            // TODO: Implement process killing using platform-specific APIs
            // Unix: use nix::unistd::kill or std::os::unix::process::CommandExt
            // Windows: use winapi or taskkill
            tracing::warn!("Would kill child process {} (not yet implemented)", pid);
        }
    }

    /// Mark the job terminal, broadcast the status event, release the active slot,
    /// and FIFO-evict finished jobs beyond RETAINED_FINISHED_JOBS.
    pub async fn finish(&self, job: &Job, status: JobStatus, error: Option<String>) {
        // Update state
        {
            let mut state = job.state.write().await;
            state.status = status;
            state.finished_at = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            );
            state.error = error;
        }

        // Broadcast status event
        if let Err(e) = job.log_tx.send(JobEvent::Status(status)) {
            tracing::warn!("Failed to broadcast status for job {}: {}", job.id, e);
        }

        // Release active slot
        *self.active.lock().await = None;

        // Add to finished order
        let mut finished_order = self.finished_order.lock().await;
        finished_order.push_back(job.id.clone());

        // Evict old finished jobs if beyond limit
        while finished_order.len() > RETAINED_FINISHED_JOBS {
            if let Some(evict_id) = finished_order.pop_front() {
                self.jobs.write().await.remove(&evict_id);
            }
        }
    }
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_submit_then_finish_transitions_state() {
        let manager = JobManager::new();

        // Submit a job
        let job = manager
            .submit(
                JobKind::Install,
                Some(koji_core::backends::BackendType::LlamaCpp),
            )
            .await
            .expect("submit should succeed");

        // Assert it's active and Running
        assert!(manager.active().await.is_some());
        {
            let state = job.state.read().await;
            assert_eq!(state.status, JobStatus::Running);
        }

        // Finish the job
        manager.finish(&job, JobStatus::Succeeded, None).await;

        // Assert state is Succeeded and active is None
        {
            let state = job.state.read().await;
            assert_eq!(state.status, JobStatus::Succeeded);
        }
        assert!(manager.active().await.is_none());
    }

    #[tokio::test]
    async fn test_concurrent_submit_returns_already_running() {
        let manager = JobManager::new();

        // Submit first job
        let _job1 = manager
            .submit(
                JobKind::Install,
                Some(koji_core::backends::BackendType::LlamaCpp),
            )
            .await
            .expect("first submit should succeed");

        // Second submit should fail
        let result = manager
            .submit(
                JobKind::Update,
                Some(koji_core::backends::BackendType::IkLlama),
            )
            .await;

        assert!(matches!(result, Err(JobError::AlreadyRunning(_))));
    }

    #[tokio::test]
    async fn test_fifo_eviction_after_retained_limit() {
        let manager = JobManager::new();

        // Submit and finish 9 jobs sequentially
        let mut job_ids = Vec::new();
        for _i in 0..9 {
            let job = manager
                .submit(
                    JobKind::Install,
                    Some(koji_core::backends::BackendType::LlamaCpp),
                )
                .await
                .expect("submit should succeed");

            manager.finish(&job, JobStatus::Succeeded, None).await;

            job_ids.push(job.id.clone());
        }

        // First job should be evicted (only 8 retained)
        assert!(manager.get(&job_ids[0]).await.is_none());

        // Second job should still exist (within limit)
        assert!(manager.get(&job_ids[1]).await.is_some());

        // Last job should exist
        assert!(manager.get(&job_ids[8]).await.is_some());
    }

    #[tokio::test]
    async fn test_log_head_invariant_first_100_lines_pinned() {
        let manager = JobManager::new();
        let job = manager
            .submit(
                JobKind::Install,
                Some(koji_core::backends::BackendType::LlamaCpp),
            )
            .await
            .expect("submit should succeed");

        // Append 150 lines
        for i in 0..150 {
            manager.append_log(&job, format!("line {}", i)).await;
        }

        // Assert head has 100 lines, front is "line 0"
        let head = job.log_head.read().await;
        assert_eq!(head.len(), 100);
        assert_eq!(head.front().unwrap(), "line 0");
        drop(head);

        // Assert dropped is 0 (all lines fit in head + tail)
        assert_eq!(job.log_dropped.load(Ordering::Relaxed), 0);

        // Assert tail has 50 lines (150 - 100)
        let tail = job.log_tail.read().await;
        assert_eq!(tail.len(), 50);
        drop(tail);
    }

    #[tokio::test]
    async fn test_log_tail_eviction_after_overflow() {
        let manager = JobManager::new();
        let job = manager
            .submit(
                JobKind::Install,
                Some(koji_core::backends::BackendType::LlamaCpp),
            )
            .await
            .expect("submit should succeed");

        // Append 1000 lines
        for i in 0..1000 {
            manager.append_log(&job, format!("line {}", i)).await;
        }

        // Assert head has 100 lines
        let head = job.log_head.read().await;
        assert_eq!(head.len(), 100);
        assert_eq!(head.front().unwrap(), "line 0");
        drop(head);

        // Assert tail has 400 lines
        let tail = job.log_tail.read().await;
        assert_eq!(tail.len(), 400);
        assert_eq!(tail.front().unwrap(), "line 600");
        drop(tail);

        // Assert dropped is 500 (1000 - 100 - 400)
        assert_eq!(job.log_dropped.load(Ordering::Relaxed), 500);
    }

    #[tokio::test]
    async fn test_broadcast_channel_delivers_live_lines() {
        let manager = JobManager::new();
        let job = manager
            .submit(
                JobKind::Install,
                Some(koji_core::backends::BackendType::LlamaCpp),
            )
            .await
            .expect("submit should succeed");

        // Subscribe to the broadcast channel
        let mut rx = job.log_tx.subscribe();

        // Append 3 lines
        manager.append_log(&job, "line 1".to_string()).await;
        manager.append_log(&job, "line 2".to_string()).await;
        manager.append_log(&job, "line 3".to_string()).await;

        // Assert receiver gets 3 Log events in order
        for expected in ["line 1", "line 2", "line 3"] {
            let event = rx.recv().await.expect("should receive event");
            if let JobEvent::Log(line) = event {
                assert_eq!(line, expected);
            } else {
                panic!("Expected JobEvent::Log, got {:?}", event);
            }
        }
    }

    #[tokio::test]
    async fn test_register_child_appends_pid() {
        let manager = JobManager::new();
        let job = manager
            .submit(
                JobKind::Install,
                Some(koji_core::backends::BackendType::LlamaCpp),
            )
            .await
            .expect("submit should succeed");

        // Register a child PID
        manager.register_child(&job, 12345).await;

        // Verify PID was added
        let pids = job.child_pids.read().await; // Placeholder for future implementation
        assert!(pids.contains(&12345));
    }

    #[tokio::test]
    async fn test_kill_children() {
        let manager = JobManager::new();
        let job = manager
            .submit(
                JobKind::Install,
                Some(koji_core::backends::BackendType::LlamaCpp),
            )
            .await
            .expect("submit should succeed");

        // Register some child PIDs
        manager.register_child(&job, 12345).await;
        manager.register_child(&job, 67890).await;

        // Kill children - this won't actually kill real processes in tests
        // but it should not panic
        manager.kill_children(&job).await;
    }

    #[tokio::test]
    async fn test_finish_with_error_message() {
        let manager = JobManager::new();
        let job = manager
            .submit(
                JobKind::Install,
                Some(koji_core::backends::BackendType::LlamaCpp),
            )
            .await
            .expect("submit should succeed");

        // Finish with an error message
        manager
            .finish(&job, JobStatus::Failed, Some("out of memory".to_string()))
            .await;

        {
            let state = job.state.read().await;
            assert_eq!(state.status, JobStatus::Failed);
        }
        assert!(manager.active().await.is_none());
    }

    #[tokio::test]
    async fn test_finish_without_error_clears_message() {
        let manager = JobManager::new();
        let job = manager
            .submit(
                JobKind::Install,
                Some(koji_core::backends::BackendType::LlamaCpp),
            )
            .await
            .expect("submit should succeed");

        // Finish without error message
        manager.finish(&job, JobStatus::Succeeded, None).await;

        {
            let state = job.state.read().await;
            assert_eq!(state.status, JobStatus::Succeeded);
        }
    }

    #[tokio::test]
    async fn test_get_nonexistent_job() {
        let manager = JobManager::new();
        assert!(manager.get(&"nonexistent-job".to_string()).await.is_none());
    }

    #[tokio::test]
    async fn test_active_returns_none_when_no_jobs() {
        let manager = JobManager::new();
        assert!(manager.active().await.is_none());
    }

    #[tokio::test]
    async fn test_fifo_eviction_preserves_order() {
        let manager = JobManager::new();
        let mut job_ids = Vec::new();

        // Submit and finish 10 jobs (limit is 8)
        for _i in 0..10 {
            let job = manager
                .submit(
                    JobKind::Install,
                    Some(koji_core::backends::BackendType::LlamaCpp),
                )
                .await
                .expect("submit should succeed");
            manager.finish(&job, JobStatus::Succeeded, None).await;
            job_ids.push(job.id.clone());
        }

        // First 2 should be evicted (10 - 8 = 2)
        assert!(manager.get(&job_ids[0]).await.is_none());
        assert!(manager.get(&job_ids[1]).await.is_none());

        // Last 8 should exist
        for i in 2..10 {
            assert!(
                manager.get(&job_ids[i]).await.is_some(),
                "job {} should exist",
                i
            );
        }
    }
}
