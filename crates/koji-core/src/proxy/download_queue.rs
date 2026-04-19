//! Download queue service and event bus for managing download lifecycle.
//!
//! Provides a `DownloadQueueService` that wraps the database query functions
//! and emits `DownloadEvent`s via a broadcast channel for each state transition.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use tokio::sync::broadcast;

use crate::db::OpenResult;

// Re-export query types for use in tests and the service.
// These are re-exported via `crate::db::queries::*`.
use crate::db::queries::{
    cancel_queue_item, get_active_items, get_item_by_job_id, get_queued_item, get_running_item,
    insert_queue_item, mark_stale_running_as_failed, try_mark_running as db_try_mark_running,
    update_queue_status, DownloadQueueItem,
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
}

impl DownloadQueueService {
    /// Create a new `DownloadQueueService` with a broadcast channel (capacity 64).
    pub fn new(db_dir: Option<PathBuf>) -> Self {
        let events_tx = broadcast::channel(64).0;
        Self { db_dir, events_tx }
    }

    /// Open a database connection using the configured db_dir.
    fn open_conn(&self) -> Result<rusqlite::Connection> {
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
    pub fn enqueue(
        &self,
        job_id: &str,
        repo_id: &str,
        filename: &str,
        display_name: Option<&str>,
        kind: &str,
    ) -> Result<()> {
        let conn = self.open_conn()?;
        insert_queue_item(&conn, job_id, repo_id, filename, display_name, kind)?;
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

    /// Subscribe to download events via a broadcast channel receiver.
    pub fn subscribe_events(&self) -> broadcast::Receiver<DownloadEvent> {
        self.events_tx.subscribe()
    }

    /// Perform startup recovery: mark stale running items as failed.
    ///
    /// For each stale item that was marked failed, emit `DownloadEvent::Failed`.
    /// Returns the list of stale job_ids so the caller can clean up in-memory state.
    pub fn on_startup_recovery(&self) -> Result<Vec<String>> {
        let conn = self.open_conn()?;

        // Get running items before marking them as failed
        let running_items = get_running_item(&conn)?
            .map(|item| vec![item])
            .unwrap_or_default();

        mark_stale_running_as_failed(&conn)?;

        let mut stale_job_ids = Vec::new();
        for item in running_items {
            if item.status == "running" || item.status == "verifying" {
                // Re-read to get updated status
                let updated = get_item_by_job_id(&conn, &item.job_id)?;
                if let Some(updated_item) = updated {
                    if updated_item.status == "failed" {
                        let _ = self.events_tx.send(DownloadEvent::Failed {
                            job_id: item.job_id.clone(),
                            filename: item.filename.clone(),
                            error: "Download was interrupted (process restart)".to_string(),
                        });
                        stale_job_ids.push(item.job_id);
                    }
                }
            }
        }

        Ok(stale_job_ids)
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

/// Start a download from the queue (stub for Task 3).
///
/// This is the ONLY code path that transitions items from `queued` → `running`.
/// The `queue_processor_loop` dequeues items and spawns this function.
async fn start_download_from_queue(svc: Arc<DownloadQueueService>, job_id: String) {
    // Read the queue item from DB to get details
    let conn = match svc.open_conn() {
        Ok(c) => c,
        Err(_) => return,
    };

    let item = match get_item_by_job_id(&conn, &job_id) {
        Ok(Some(item)) => item,
        _ => return,
    };

    let start = Instant::now();

    // Emit Verifying event (simulating download completion → verification)
    let _ = svc.events_tx.send(DownloadEvent::Verifying {
        job_id: job_id.clone(),
        filename: item.filename.clone(),
    });

    // Simulate a short delay for the stub
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let duration_ms = start.elapsed().as_millis() as u64;

    // Mark as completed
    let _ = svc.update_status(
        &job_id,
        "completed",
        item.bytes_downloaded,
        item.total_bytes,
        None,
        Some(duration_ms),
    );
}

/// Background processor loop that picks up queued items one at a time.
///
/// This is the ONLY code path that transitions items from `queued` → `running`.
pub(crate) async fn queue_processor_loop(svc: Arc<DownloadQueueService>) {
    // Startup recovery: mark stale running items as failed
    if let Err(e) = svc.on_startup_recovery() {
        tracing::error!(error=%e, "Startup recovery failed");
    }

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Check if anything is currently running (only one at a time in sequential mode)
        let active = match svc.get_active_items() {
            Ok(items) => items,
            Err(e) => {
                tracing::error!(error=%e, "Failed to check active downloads");
                continue;
            }
        };

        let has_running = active
            .iter()
            .any(|item| item.status == "running" || item.status == "verifying");

        if has_running {
            continue; // Something is running, wait for it to finish
        }

        // Try to dequeue the next item
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
            let svc_clone = Arc::clone(&svc);
            tokio::spawn(async move {
                start_download_from_queue(svc_clone, job_id).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_service() -> DownloadQueueService {
        // We need a temp directory for the service to work (open_conn uses db_dir)
        let tmp = tempfile::tempdir().unwrap();
        let svc = DownloadQueueService::new(Some(tmp.path().to_path_buf()));
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
}
