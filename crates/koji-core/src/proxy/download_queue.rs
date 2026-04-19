//! Download queue service and event bus for managing download lifecycle.
//!
//! Provides a `DownloadQueueService` that wraps the database query functions
//! and emits `DownloadEvent`s via a broadcast channel for each state transition.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::broadcast;

use crate::db::OpenResult;

// Re-export query types for use in tests and the service.
// These are re-exported via `crate::db::queries::*`.
use crate::db::queries::{
    cancel_queue_item, count_history_items, get_active_items, get_history_items,
    get_item_by_job_id, get_queued_item, insert_queue_item, mark_stale_running_as_queued,
    try_mark_running as db_try_mark_running, update_progress_only, update_queue_status,
    DownloadQueueItem,
};

/// Events emitted by the download queue service during lifecycle transitions.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Started {
        job_id: String,
        repo_id: String,
        filename: String,
        total_bytes: Option<u64>,
    },
    Progress {
        job_id: String,
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
    },
    Verifying {
        job_id: String,
        filename: String,
    },
    Completed {
        job_id: String,
        filename: String,
        size_bytes: u64,
        duration_ms: u64,
    },
    Failed {
        job_id: String,
        filename: String,
        error: String,
    },
    Cancelled {
        job_id: String,
        filename: String,
    },
    Queued {
        job_id: String,
        repo_id: String,
        filename: String,
    },
}

/// Service that manages the download queue lifecycle.
pub struct DownloadQueueService {
    db_dir: Option<PathBuf>,
    events_tx: broadcast::Sender<DownloadEvent>,
    poll_interval_secs: u64,
}

impl DownloadQueueService {
    /// Create a new `DownloadQueueService` with a broadcast channel.
    ///
    /// Capacity is set to 256 to accommodate rapid progress updates during
    /// large downloads without dropping events. The SSE endpoint handles
    /// dropped events via the `Lagged` marker event.
    pub fn new(db_dir: Option<PathBuf>, poll_interval_secs: u64) -> Self {
        let events_tx = broadcast::channel(256).0;
        Self {
            db_dir,
            events_tx,
            poll_interval_secs,
        }
    }

    /// Open a database connection using the configured db_dir.
    pub fn open_conn(&self) -> Result<rusqlite::Connection> {
        let dir = self
            .db_dir
            .as_ref()
            .ok_or_else(|| anyhow!("Database directory not configured"))?;
        let OpenResult { conn, .. } = crate::db::open(dir)?;
        Ok(conn)
    }

    /// Enqueue a new download item.
    ///
    /// Opens a DB connection, inserts the queue item, and emits `DownloadEvent::Queued`.
    /// Returns `Err` if the job_id already exists (UNIQUE constraint violation).
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue(
        &self,
        job_id: &str,
        repo_id: &str,
        filename: &str,
        display_name: Option<&str>,
        kind: &str,
        quant: Option<&str>,
        context_length: Option<u32>,
    ) -> Result<()> {
        let conn = self.open_conn()?;
        insert_queue_item(
            &conn,
            job_id,
            repo_id,
            filename,
            display_name,
            kind,
            quant,
            context_length,
        )?;
        let _ = self.events_tx.send(DownloadEvent::Queued {
            job_id: job_id.to_string(),
            repo_id: repo_id.to_string(),
            filename: filename.to_string(),
        });
        Ok(())
    }

    /// Dequeue the oldest queued item (FIFO).
    ///
    /// Opens a DB connection and returns the next item, or `None` if empty.
    pub fn dequeue(&self) -> Result<Option<DownloadQueueItem>> {
        let conn = self.open_conn()?;
        get_queued_item(&conn)
    }

    /// Update a queue item's status and emit the corresponding event.
    ///
    /// Reads the current row to get filename/repo_id for event emission,
    /// then updates the status in the DB.
    pub fn update_status(
        &self,
        job_id: &str,
        new_status: &str,
        bytes_downloaded: i64,
        total_bytes: Option<i64>,
        error_message: Option<&str>,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        let conn = self.open_conn()?;
        let item = get_item_by_job_id(&conn, job_id)?
            .ok_or_else(|| anyhow!("Job '{}' not found", job_id))?;

        update_queue_status(
            &conn,
            job_id,
            new_status,
            bytes_downloaded,
            total_bytes,
            error_message,
        )?;

        let event = match new_status {
            // Note: "progress" is intentionally not handled here. Progress events
            // are emitted by update_progress() which uses update_progress_only()
            // directly, avoiding any status field changes.
            "running" => DownloadEvent::Started {
                job_id: job_id.to_string(),
                repo_id: item.repo_id.clone(),
                filename: item.filename.clone(),
                total_bytes: total_bytes.map(|b| b as u64),
            },
            "verifying" => DownloadEvent::Verifying {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
            },
            "completed" => DownloadEvent::Completed {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
                size_bytes: bytes_downloaded as u64,
                duration_ms: duration_ms.unwrap_or(0),
            },
            "failed" => DownloadEvent::Failed {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
                error: error_message.unwrap_or("Unknown error").to_string(),
            },
            "cancelled" => DownloadEvent::Cancelled {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
            },
            _ => return Ok(()),
        };

        let _ = self.events_tx.send(event);
        Ok(())
    }

    /// Update only progress fields without changing status.
    ///
    /// Emits `DownloadEvent::Progress` and updates bytes_downloaded/total_bytes
    /// in the DB without overwriting the current status (running/verifying).
    pub fn update_progress(
        &self,
        job_id: &str,
        bytes_downloaded: i64,
        total_bytes: Option<i64>,
    ) -> Result<()> {
        let conn = self.open_conn()?;
        update_progress_only(&conn, job_id, bytes_downloaded, total_bytes)?;

        let _ = self.events_tx.send(DownloadEvent::Progress {
            job_id: job_id.to_string(),
            bytes_downloaded: bytes_downloaded as u64,
            total_bytes: total_bytes.map(|b| b as u64),
        });
        Ok(())
    }

    /// Cancel a queue item if it hasn't reached a terminal state.
    ///
    /// Opens a DB connection, cancels the item, and emits `DownloadEvent::Cancelled`.
    pub fn cancel(&self, job_id: &str) -> Result<()> {
        let conn = self.open_conn()?;

        // Check if the item exists and is in a non-terminal state
        let item = get_item_by_job_id(&conn, job_id)?
            .ok_or_else(|| anyhow!("Job '{}' not found", job_id))?;

        if matches!(item.status.as_str(), "completed" | "failed" | "cancelled") {
            return Err(anyhow!(
                "Job '{}' is already in terminal state '{}'",
                job_id,
                item.status
            ));
        }

        cancel_queue_item(&conn, job_id)?;

        let _ = self.events_tx.send(DownloadEvent::Cancelled {
            job_id: job_id.to_string(),
            filename: item.filename.clone(),
        });
        Ok(())
    }

    /// Get all active items (queued + running + verifying), ordered by status priority.
    pub fn get_active_items(&self) -> Result<Vec<DownloadQueueItem>> {
        let conn = self.open_conn()?;
        get_active_items(&conn)
    }

    /// Get history items (completed, failed, cancelled), sorted newest first.
    pub fn get_history_items(&self, limit: i64, offset: i64) -> Result<Vec<DownloadQueueItem>> {
        let conn = self.open_conn()?;
        get_history_items(&conn, limit, offset)
    }

    /// Count total history items (completed, failed, cancelled).
    pub fn count_history_items(&self) -> Result<i64> {
        let conn = self.open_conn()?;
        count_history_items(&conn)
    }

    /// Subscribe to download events via a broadcast channel receiver.
    pub fn subscribe_events(&self) -> broadcast::Receiver<DownloadEvent> {
        self.events_tx.subscribe()
    }

    /// Perform startup recovery: re-queue stale running items so they get retried.
    ///
    /// Clears started_at so the download restarts fresh (hf-hub resumes if the
    /// partial file exists on disk, otherwise it downloads from scratch).
    pub fn on_startup_recovery(&self) -> Result<()> {
        let conn = self.open_conn()?;
        mark_stale_running_as_queued(&conn)?;
        Ok(())
    }

    /// Atomically claim a queued item as running.
    ///
    /// Returns `true` if the item was claimed (was queued, now running),
    /// `false` if it was already started by someone else.
    pub fn try_mark_running(&self, job_id: &str) -> Result<bool> {
        let conn = self.open_conn()?;
        db_try_mark_running(&conn, job_id)
    }
}

/// Start a download from the queue.
///
/// This is the ONLY code path that transitions items from `queued` → `running`.
/// Reads the queued item from DB, constructs a QuantDownloadSpec, and calls
/// the real download implementation from pull.rs.
async fn start_download_from_queue(
    state: Arc<super::ProxyState>,
    svc: Arc<DownloadQueueService>,
    job_id: String,
) {
    // Read the queue item from DB to get details
    let conn = match svc.open_conn() {
        Ok(c) => c,
        Err(_) => return,
    };

    let item = match get_item_by_job_id(&conn, &job_id) {
        Ok(Some(item)) => item,
        _ => return,
    };

    // Construct QuantDownloadSpec from DB data
    let spec = super::koji_handlers::QuantDownloadSpec {
        filename: item.filename.clone(),
        quant: item.quant.clone(),
        context_length: item.context_length,
    };

    // Delegate to the real download implementation in pull.rs.
    // Note: the caller (queue_processor_loop) already spawned a task,
    // so we call directly without another spawn.
    super::koji_handlers::start_download_from_queue(
        state,
        job_id,
        item.repo_id,
        item.filename,
        spec,
    )
    .await;
}

/// Background processor loop that picks up queued items one at a time.
///
/// This is the ONLY code path that transitions items from `queued` → `running`.
pub(crate) async fn queue_processor_loop(state: Arc<super::ProxyState>) {
    let svc = state
        .download_queue
        .as_ref()
        .expect("download_queue must be configured");

    // Startup recovery: mark stale running items as failed
    if let Err(e) = svc.on_startup_recovery() {
        tracing::error!(error=%e, "Startup recovery failed");
    }

    let poll_interval = std::cmp::max(svc.poll_interval_secs, 1);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;

        // Check if anything is currently running (only one at a time in sequential mode)
        let active = match svc.get_active_items() {
            Ok(items) => items,
            Err(e) => {
                tracing::error!(error=%e, "Failed to check active downloads");
                continue;
            }
        };

        // Find any running or verifying item
        let running_item = active.iter().find(|i| {
            i.status == "running" || i.status == "verifying"
        });

        if let Some(item) = running_item {
            // Something is supposedly running. Check if it's actually alive.
            // Use the pull_jobs in-memory state to detect stale downloads.
            let is_alive = {
                let jobs = state.pull_jobs.read().await;
                jobs.contains_key(&item.job_id)
            };
            if is_alive {
                continue; // Download is alive, wait for it to finish
            }
            // Download task died without reaching terminal state. Re-queue it so
            // the processor picks it up and retries (hf-hub resumes if file exists).
            tracing::warn!(job_id=%item.job_id, "Download task died without reaching terminal state, re-queuing");
            let conn = match svc.open_conn() {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error=%e, "Failed to open conn for re-queue");
                    continue;
                }
            };
            if let Err(e) = mark_stale_running_as_queued(&conn) {
                tracing::error!(error=%e, job_id=%item.job_id, "Failed to re-queue stale item");
            }
            // Don't fall through — the re-queued item will be picked up on next poll.
            // We need it to go through dequeue() → try_mark_running() to properly restart.
            continue;
        }

        // Try to dequeue the next queued item
        let Some(item) = (match svc.dequeue() {
            Ok(item) => item,
            Err(e) => {
                tracing::error!(error=%e, "Failed to dequeue next item");
                continue;
            }
        }) else {
            // queue empty, continue looping
            continue;
        };

        // Atomic CAS: only transition if still 'queued'. This is the safety guard
        // that prevents double-starts. If another consumer already marked it running,
        // this returns false and we skip.
        let was_queued = match svc.try_mark_running(&item.job_id) {
            Ok(true) => true,
            Ok(false) => {
                tracing::info!(
                    job_id = %item.job_id,
                    "Item already started by another consumer, skipping"
                );
                continue;
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    job_id = %item.job_id,
                    "CAS failed to mark item as running"
                );
                continue;
            }
        };

        if was_queued {
            // Emit Started event (reads filename from DB via update_status)
            let _ = svc.update_status(&item.job_id, "running", 0, None, None, None);
            // Spawn the actual download (delegated to a separate async function)
            let job_id = item.job_id.clone();
            let state_clone = Arc::clone(&state);
            let svc_clone = Arc::clone(svc);
            tokio::spawn(async move {
                start_download_from_queue(state_clone, svc_clone, job_id).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn setup_service() -> DownloadQueueService {
        // We need a temp directory for the service to work (open_conn uses db_dir)
        let tmp = tempfile::tempdir().unwrap();
        let svc = DownloadQueueService::new(Some(tmp.path().to_path_buf()), 2);
        // Open and initialize the DB once
        let _ = svc.open_conn().unwrap();
        svc
    }

    #[test]
    fn test_enqueue_and_dequeue() {
        let svc = setup_service();

        svc.enqueue(
            "job-1",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();

        let item = svc.dequeue().unwrap().unwrap();
        assert_eq!(item.job_id, "job-1");
        assert_eq!(item.repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
        assert_eq!(item.filename, "Qwen3.6-35B-Q4_K_M.gguf");
        assert_eq!(item.display_name, Some("Qwen3.6 35B".to_string()));
        assert_eq!(item.status, "queued");
        assert_eq!(item.kind, "model");
    }

    #[test]
    fn test_update_status_emits_event() {
        let svc = setup_service();

        svc.enqueue(
            "job-1",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();

        let mut rx = svc.subscribe_events();

        svc.update_status("job-1", "running", 0, Some(2000), None, None)
            .unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Started {
                job_id,
                repo_id,
                filename,
                total_bytes,
            } => {
                assert_eq!(job_id, "job-1");
                assert_eq!(repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
                assert_eq!(filename, "Qwen3.6-35B-Q4_K_M.gguf");
                assert_eq!(total_bytes, Some(2000));
            }
            other => panic!("Expected Started event, got {:?}", other),
        }
    }

    #[test]
    fn test_cancel_emits_event() {
        let svc = setup_service();

        svc.enqueue(
            "job-1",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();

        let mut rx = svc.subscribe_events();

        svc.cancel("job-1").unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Cancelled { job_id, filename } => {
                assert_eq!(job_id, "job-1");
                assert_eq!(filename, "Qwen3.6-35B-Q4_K_M.gguf");
            }
            other => panic!("Expected Cancelled event, got {:?}", other),
        }
    }

    #[test]
    fn test_dequeue_empty_queue_returns_none() {
        let svc = setup_service();

        let result = svc.dequeue().unwrap();
        assert!(result.is_none());
    }

    /// Integration test: verify that enqueue_download creates a download_queue row
    /// with the correct fields including quant and context_length.
    #[test]
    fn test_enqueue_download_creates_queue_row() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DownloadQueueService::new(Some(tmp.path().to_path_buf()), 2);
        let _ = svc.open_conn().unwrap();

        // Subscribe before enqueue so we can receive the event
        let mut rx = svc.subscribe_events();

        svc.enqueue(
            "pull-test-001",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();

        // Verify the row exists in the DB
        let conn = svc.open_conn().unwrap();
        let item = crate::db::queries::get_item_by_job_id(&conn, "pull-test-001")
            .unwrap()
            .expect("row should exist");

        assert_eq!(item.job_id, "pull-test-001");
        assert_eq!(item.status, "queued");
        assert_eq!(item.quant, Some("Q4_K_M".to_string()));
        assert_eq!(item.context_length, Some(4096));

        // Verify the Queued event was emitted
        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Queued {
                job_id,
                repo_id,
                filename,
            } => {
                assert_eq!(job_id, "pull-test-001");
                assert_eq!(repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
                assert_eq!(filename, "Qwen3.6-35B-Q4_K_M.gguf");
            }
            other => panic!("Expected Queued event, got {:?}", other),
        }
    }

    /// Integration test: verify full lifecycle status transitions through the DB.
    #[test]
    fn test_status_transitions_through_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DownloadQueueService::new(Some(tmp.path().to_path_buf()), 2);
        let _ = svc.open_conn().unwrap();

        // Subscribe before enqueue so we can receive events
        let mut rx = svc.subscribe_events();

        // Step 1: Enqueue
        svc.enqueue(
            "pull-test-002",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            Some(2048),
        )
        .unwrap();

        let conn = svc.open_conn().unwrap();
        let item = crate::db::queries::get_item_by_job_id(&conn, "pull-test-002")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "queued");

        // Step 2: Transition to running
        svc.update_status("pull-test-002", "running", 0, None, None, None)
            .unwrap();

        let item = crate::db::queries::get_item_by_job_id(&conn, "pull-test-002")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "running");
        assert!(item.started_at.is_some());

        // Step 3: Transition to verifying
        svc.update_status("pull-test-002", "verifying", 1000, Some(2000), None, None)
            .unwrap();

        let item = crate::db::queries::get_item_by_job_id(&conn, "pull-test-002")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "verifying");

        // Step 4: Transition to completed with duration
        let start = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let duration_ms = start.elapsed().as_millis() as u64;

        svc.update_status(
            "pull-test-002",
            "completed",
            2000,
            Some(2000),
            None,
            Some(duration_ms),
        )
        .unwrap();

        let item = crate::db::queries::get_item_by_job_id(&conn, "pull-test-002")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "completed");
        assert!(item.completed_at.is_some());

        // Drain any intermediate events and find the Completed event
        let mut completed_event = None;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, DownloadEvent::Completed { .. }) {
                completed_event = Some(event);
            }
        }
        let event = completed_event.expect("Expected Completed event");
        match event {
            DownloadEvent::Completed {
                job_id,
                filename,
                size_bytes,
                duration_ms: event_duration,
            } => {
                assert_eq!(job_id, "pull-test-002");
                assert_eq!(filename, "model.gguf");
                assert_eq!(size_bytes, 2000);
                assert!(
                    event_duration >= duration_ms,
                    "event duration {} should be >= computed {}",
                    event_duration,
                    duration_ms
                );
            }
            other => panic!("Expected Completed event, got {:?}", other),
        }
    }

    /// Integration test: verify duration_ms is computed via Instant::elapsed()
    /// and not derived from string subtraction of timestamps.
    #[test]
    fn test_duration_ms_computed_via_instant() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = DownloadQueueService::new(Some(tmp.path().to_path_buf()), 2);
        let _ = svc.open_conn().unwrap();

        // Subscribe before enqueue so we can receive events
        let mut rx = svc.subscribe_events();

        // Enqueue the item
        svc.enqueue(
            "pull-test-003",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Transition through the lifecycle with known delays
        svc.update_status("pull-test-003", "running", 0, None, None, None)
            .unwrap();

        // Sleep for a known duration, then compute duration via Instant::elapsed()
        let start = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(15));
        let computed_duration = start.elapsed().as_millis() as u64;

        svc.update_status(
            "pull-test-003",
            "completed",
            5000,
            Some(5000),
            None,
            Some(computed_duration),
        )
        .unwrap();

        // Drain any intermediate events and find the Completed event
        let mut completed_event = None;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, DownloadEvent::Completed { .. }) {
                completed_event = Some(event);
            }
        }
        let event = completed_event.expect("Expected Completed event");
        match event {
            DownloadEvent::Completed { duration_ms, .. } => {
                assert!(
                    duration_ms >= computed_duration,
                    "duration_ms ({}) should be >= computed ({})",
                    duration_ms,
                    computed_duration
                );
            }
            other => panic!("Expected Completed event, got {:?}", other),
        }

        // Verify the DB row has completed_at set (timestamp-based), but
        // duration_ms was computed in Rust via Instant::elapsed()
        let conn = svc.open_conn().unwrap();
        let item = crate::db::queries::get_item_by_job_id(&conn, "pull-test-003")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "completed");
        assert!(item.completed_at.is_some());
    }
}
