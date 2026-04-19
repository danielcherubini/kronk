//! Query functions for the `download_queue` table.
//!
//! All functions take a `&Connection` — the caller owns the connection.
//! All functions are synchronous (no async).

use anyhow::Result;
use rusqlite::Connection;

/// A row from the download_queue table.
#[derive(Debug, Clone)]
pub struct DownloadQueueItem {
    pub id: i64,
    pub job_id: String,
    pub repo_id: String,
    pub filename: String,
    pub display_name: Option<String>,
    pub status: String, // "queued" | "running" | "verifying" | "completed" | "failed" | "cancelled"
    pub bytes_downloaded: i64,
    pub total_bytes: Option<i64>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub queued_at: String,
    pub kind: String, // "model" | "backend"
    pub quant: Option<String>,
    pub context_length: Option<u32>,
}

/// Column indices for the download_queue table (used by query helpers).
mod cols {
    pub const ID: usize = 0;
    pub const JOB_ID: usize = 1;
    pub const REPO_ID: usize = 2;
    pub const FILENAME: usize = 3;
    pub const DISPLAY_NAME: usize = 4;
    pub const STATUS: usize = 5;
    pub const BYTES_DOWNLOADED: usize = 6;
    pub const TOTAL_BYTES: usize = 7;
    pub const ERROR_MESSAGE: usize = 8;
    pub const STARTED_AT: usize = 9;
    pub const COMPLETED_AT: usize = 10;
    pub const QUEUED_AT: usize = 11;
    pub const KIND: usize = 12;
    pub const QUANT: usize = 13;
    pub const CONTEXT_LENGTH: usize = 14;
}

/// Helper to map a SQL row to a `DownloadQueueItem`.
pub fn map_queue_item(row: &rusqlite::Row) -> rusqlite::Result<DownloadQueueItem> {
    Ok(DownloadQueueItem {
        id: row.get(cols::ID)?,
        job_id: row.get(cols::JOB_ID)?,
        repo_id: row.get(cols::REPO_ID)?,
        filename: row.get(cols::FILENAME)?,
        display_name: row.get(cols::DISPLAY_NAME)?,
        status: row.get(cols::STATUS)?,
        bytes_downloaded: row.get(cols::BYTES_DOWNLOADED)?,
        total_bytes: row.get(cols::TOTAL_BYTES)?,
        error_message: row.get(cols::ERROR_MESSAGE)?,
        started_at: row.get(cols::STARTED_AT)?,
        completed_at: row.get(cols::COMPLETED_AT)?,
        queued_at: row.get(cols::QUEUED_AT)?,
        kind: row.get(cols::KIND)?,
        quant: row.get(cols::QUANT)?,
        context_length: row.get(cols::CONTEXT_LENGTH)?,
    })
}

/// Insert a new item into the download queue.
/// Returns the new row id.
#[allow(clippy::too_many_arguments)]
pub fn insert_queue_item(
    conn: &Connection,
    job_id: &str,
    repo_id: &str,
    filename: &str,
    display_name: Option<&str>,
    kind: &str,
    quant: Option<&str>,
    context_length: Option<u32>,
) -> Result<i64> {
    let id = conn.execute(
        "INSERT INTO download_queue \
         (job_id, repo_id, filename, display_name, status, kind, queued_at, quant, context_length) \
         VALUES (?1, ?2, ?3, ?4, 'queued', ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?6, ?7)",
        (
            job_id,
            repo_id,
            filename,
            display_name,
            kind,
            quant,
            context_length,
        ),
    )?;
    Ok(id as i64)
}

/// Retrieve the oldest queued item (FIFO).
pub fn get_queued_item(conn: &Connection) -> Result<Option<DownloadQueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, repo_id, filename, display_name, status, \
                bytes_downloaded, total_bytes, error_message, started_at, \
                completed_at, queued_at, kind, quant, context_length \
         FROM download_queue \
         WHERE status = 'queued' \
         ORDER BY queued_at ASC \
         LIMIT 1",
    )?;
    let mut rows = stmt.query_map([], map_queue_item)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Update a queue item's status and related fields.
///
/// - `started_at` is set only if it's currently NULL (first time going to running).
/// - `completed_at` is set only when transitioning to a terminal state
///   (completed, failed, cancelled).
pub fn update_queue_status(
    conn: &Connection,
    job_id: &str,
    new_status: &str,
    bytes_downloaded: i64,
    total_bytes: Option<i64>,
    error_message: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE download_queue SET \
         status = ?1, \
         bytes_downloaded = ?2, \
         total_bytes = ?3, \
         error_message = ?4, \
         started_at = COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), \
         completed_at = CASE WHEN ?5 IN ('completed','failed','cancelled') \
             THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE completed_at END \
         WHERE job_id = ?6",
        (
            new_status,
            bytes_downloaded,
            total_bytes,
            error_message,
            new_status,
            job_id,
        ),
    )?;
    Ok(())
}

/// Update only the progress fields (bytes_downloaded, total_bytes) without
/// changing the status. Used for real-time progress streaming via SSE.
pub fn update_progress_only(
    conn: &Connection,
    job_id: &str,
    bytes_downloaded: i64,
    total_bytes: Option<i64>,
) -> Result<()> {
    conn.execute(
        "UPDATE download_queue SET \
         bytes_downloaded = ?1, \
         total_bytes = ?2 \
         WHERE job_id = ?3",
        (bytes_downloaded, total_bytes, job_id),
    )?;
    Ok(())
}

/// Atomically claim a queued item as running.
///
/// Returns `true` if a row was affected (item was queued, now running),
/// `false` if no row matched (item already started by someone else).
/// This is the atomic CAS guard that prevents double-starting downloads.
pub fn try_mark_running(conn: &Connection, job_id: &str) -> Result<bool> {
    let rows = conn.execute(
        "UPDATE download_queue SET \
         status = 'running', \
         started_at = COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) \
         WHERE job_id = ?1 AND status = 'queued'",
        (job_id,),
    )?;
    Ok(rows > 0)
}

/// Retrieve a queue item by its job_id.
pub fn get_item_by_job_id(conn: &Connection, job_id: &str) -> Result<Option<DownloadQueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, repo_id, filename, display_name, status, \
                bytes_downloaded, total_bytes, error_message, started_at, \
                completed_at, queued_at, kind, quant, context_length \
         FROM download_queue \
         WHERE job_id = ?1 \
         LIMIT 1",
    )?;
    let mut rows = stmt.query_map((job_id,), map_queue_item)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Get all active items (queued, running, verifying), ordered by status priority then queued_at.
pub fn get_active_items(conn: &Connection) -> Result<Vec<DownloadQueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, repo_id, filename, display_name, status, \
                bytes_downloaded, total_bytes, error_message, started_at, \
                completed_at, queued_at, kind, quant, context_length \
         FROM download_queue \
         WHERE status IN ('queued', 'running', 'verifying') \
         ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'verifying' THEN 1 ELSE 2 END, \
                  queued_at ASC",
    )?;
    let rows = stmt.query_map([], map_queue_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Get history items (completed, failed, cancelled), sorted newest first.
pub fn get_history_items(
    conn: &Connection,
    limit: i64,
    offset: i64,
) -> Result<Vec<DownloadQueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, repo_id, filename, display_name, status, \
                bytes_downloaded, total_bytes, error_message, started_at, \
                completed_at, queued_at, kind, quant, context_length \
         FROM download_queue \
         WHERE status IN ('completed', 'failed', 'cancelled') \
         ORDER BY completed_at DESC \
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt.query_map((limit, offset), map_queue_item)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Count total history items (completed, failed, cancelled).
pub fn count_history_items(conn: &Connection) -> Result<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM download_queue \
         WHERE status IN ('completed', 'failed', 'cancelled')",
        [],
        |row| row.get(0),
    )?;
    Ok(count)
}

/// Cancel a queue item if it hasn't reached a terminal state.
pub fn cancel_queue_item(conn: &Connection, job_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE download_queue SET \
         status = 'cancelled', \
         completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE job_id = ?1 AND status IN ('queued', 'running', 'verifying')",
        (job_id,),
    )?;
    Ok(())
}

/// Get the currently running item (if any).
pub fn get_running_item(conn: &Connection) -> Result<Option<DownloadQueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, repo_id, filename, display_name, status, \
                bytes_downloaded, total_bytes, error_message, started_at, \
                completed_at, queued_at, kind, quant, context_length \
         FROM download_queue \
         WHERE status IN ('running', 'verifying') \
         LIMIT 1",
    )?;
    let mut rows = stmt.query_map([], map_queue_item)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Get all currently running/verifying items.
pub fn get_all_running_items(conn: &Connection) -> Result<Vec<DownloadQueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, repo_id, filename, display_name, status, \
                bytes_downloaded, total_bytes, error_message, started_at, \
                completed_at, queued_at, kind, quant, context_length \
         FROM download_queue \
         WHERE status IN ('running', 'verifying')",
    )?;
    let rows = stmt.query_map([], map_queue_item)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Mark stale running items as failed (process died without completing).
pub fn mark_stale_running_as_failed(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE download_queue SET \
         status = 'failed', \
         error_message = 'Download was interrupted (process restart)', \
         completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE status IN ('running', 'verifying') AND completed_at IS NULL",
        [],
    )?;
    Ok(())
}

/// Mark stale running items as queued so they get retried on next poll.
/// Clears started_at so the download restarts fresh (hf-hub resumes if file exists).
pub fn mark_stale_running_as_queued(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE download_queue SET \
         status = 'queued', \
         started_at = NULL \
         WHERE status IN ('running', 'verifying') AND completed_at IS NULL",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    fn setup() -> Connection {
        let result = open_in_memory().unwrap();
        result.conn
    }

    #[test]
    fn test_insert_and_get_queued() {
        let conn = setup();

        let id = insert_queue_item(
            &conn,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();
        assert!(id > 0);

        let item = get_queued_item(&conn).unwrap().unwrap();
        assert_eq!(item.job_id, "pull-abc123");
        assert_eq!(item.repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
        assert_eq!(item.filename, "Qwen3.6-35B-A3B-Q4_K_M.gguf");
        assert_eq!(item.display_name, Some("Qwen3.6 35B".to_string()));
        assert_eq!(item.status, "queued");
        assert_eq!(item.kind, "model");
        assert_eq!(item.quant, Some("Q4_K_M".to_string()));
        assert_eq!(item.context_length, Some(4096));
    }

    #[test]
    fn test_update_status_sets_timestamps() {
        let conn = setup();

        insert_queue_item(
            &conn,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Update to running — started_at should be set
        update_queue_status(&conn, "pull-abc123", "running", 0, None, None).unwrap();
        let item = get_item_by_job_id(&conn, "pull-abc123").unwrap().unwrap();
        assert_eq!(item.status, "running");
        assert!(
            item.started_at.is_some(),
            "started_at should be set when going to running"
        );
        assert!(
            item.completed_at.is_none(),
            "completed_at should not be set when going to running"
        );

        // Update to completed — completed_at should be set
        update_queue_status(&conn, "pull-abc123", "completed", 1000, Some(2000), None).unwrap();
        let item = get_item_by_job_id(&conn, "pull-abc123").unwrap().unwrap();
        assert_eq!(item.status, "completed");
        assert!(
            item.completed_at.is_some(),
            "completed_at should be set when going to completed"
        );
    }

    #[test]
    fn test_get_active_items_ordering() {
        let conn = setup();

        // Insert items in various statuses
        insert_queue_item(
            &conn,
            "pull-1",
            "repo/1",
            "file1.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        update_queue_status(&conn, "pull-1", "queued", 0, None, None).unwrap();

        insert_queue_item(
            &conn,
            "pull-2",
            "repo/2",
            "file2.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        update_queue_status(&conn, "pull-2", "running", 500, Some(1000), None).unwrap();

        insert_queue_item(
            &conn,
            "pull-3",
            "repo/3",
            "file3.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        update_queue_status(&conn, "pull-3", "verifying", 1000, Some(1000), None).unwrap();

        let items = get_active_items(&conn).unwrap();
        assert_eq!(items.len(), 3);
        // Running should come first, then verifying, then queued
        assert_eq!(items[0].status, "running");
        assert_eq!(items[1].status, "verifying");
        assert_eq!(items[2].status, "queued");
    }

    #[test]
    fn test_cancel_queue_item() {
        let conn = setup();

        insert_queue_item(
            &conn,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        cancel_queue_item(&conn, "pull-abc123").unwrap();

        let item = get_item_by_job_id(&conn, "pull-abc123").unwrap().unwrap();
        assert_eq!(item.status, "cancelled");
        assert!(
            item.completed_at.is_some(),
            "completed_at should be set on cancel"
        );
    }

    #[test]
    fn test_cancel_does_not_affect_completed() {
        let conn = setup();

        insert_queue_item(
            &conn,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Mark as completed first
        update_queue_status(&conn, "pull-abc123", "completed", 1000, Some(2000), None).unwrap();

        // Try to cancel — should have no effect
        cancel_queue_item(&conn, "pull-abc123").unwrap();

        let item = get_item_by_job_id(&conn, "pull-abc123").unwrap().unwrap();
        assert_eq!(
            item.status, "completed",
            "completed items should not be cancelled"
        );
    }

    #[test]
    fn test_get_history_items() {
        let conn = setup();

        // Insert completed item first
        insert_queue_item(
            &conn,
            "pull-1",
            "repo/1",
            "file1.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        update_queue_status(&conn, "pull-1", "completed", 1000, Some(2000), None).unwrap();

        // Small delay to ensure different completed_at timestamps
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Insert failed item second
        insert_queue_item(
            &conn,
            "pull-2",
            "repo/2",
            "file2.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        update_queue_status(
            &conn,
            "pull-2",
            "failed",
            500,
            Some(1000),
            Some("connection error"),
        )
        .unwrap();

        let items = get_history_items(&conn, 10, 0).unwrap();
        assert_eq!(items.len(), 2);
        // Should be sorted by completed_at DESC (newest first)
        assert_eq!(items[0].status, "failed");
        assert_eq!(items[1].status, "completed");
    }

    #[test]
    fn test_count_history_items() {
        let conn = setup();

        // Insert items with various terminal statuses
        insert_queue_item(
            &conn,
            "pull-1",
            "repo/1",
            "file1.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        update_queue_status(&conn, "pull-1", "completed", 1000, Some(2000), None).unwrap();

        insert_queue_item(
            &conn,
            "pull-2",
            "repo/2",
            "file2.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        update_queue_status(&conn, "pull-2", "failed", 500, Some(1000), Some("error")).unwrap();

        insert_queue_item(
            &conn,
            "pull-3",
            "repo/3",
            "file3.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        update_queue_status(&conn, "pull-3", "cancelled", 0, None, None).unwrap();

        // Insert a non-terminal item — should not be counted
        insert_queue_item(
            &conn,
            "pull-4",
            "repo/4",
            "file4.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();

        let count = count_history_items(&conn).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_mark_stale_running_as_failed() {
        let conn = setup();

        insert_queue_item(
            &conn,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Manually set to running without completed_at (simulates process crash)
        conn.execute(
            "UPDATE download_queue SET status = 'running', started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE job_id = 'pull-abc123'",
            [],
        )
        .unwrap();

        mark_stale_running_as_failed(&conn).unwrap();

        let item = get_item_by_job_id(&conn, "pull-abc123").unwrap().unwrap();
        assert_eq!(item.status, "failed");
        assert!(item.completed_at.is_some());
        assert_eq!(
            item.error_message.as_deref(),
            Some("Download was interrupted (process restart)")
        );
    }

    #[test]
    fn test_try_mark_running_succeeds() {
        let conn = setup();

        insert_queue_item(
            &conn,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        let claimed = try_mark_running(&conn, "pull-abc123").unwrap();
        assert!(claimed, "should return true when claiming a queued item");

        let item = get_item_by_job_id(&conn, "pull-abc123").unwrap().unwrap();
        assert_eq!(item.status, "running");
    }

    #[test]
    fn test_try_mark_running_fails_if_already_started() {
        let conn = setup();

        insert_queue_item(
            &conn,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Manually set to running so it's not queued anymore
        conn.execute(
            "UPDATE download_queue SET status = 'running', started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE job_id = 'pull-abc123'",
            [],
        )
        .unwrap();

        let claimed = try_mark_running(&conn, "pull-abc123").unwrap();
        assert!(!claimed, "should return false when item is already running");

        let item = get_item_by_job_id(&conn, "pull-abc123").unwrap().unwrap();
        assert_eq!(item.status, "running", "status should remain unchanged");
    }

    #[test]
    fn test_get_item_by_job_id() {
        let conn = setup();

        insert_queue_item(
            &conn,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        let item = get_item_by_job_id(&conn, "pull-abc123").unwrap().unwrap();
        assert!(item.id > 0);
        assert_eq!(item.job_id, "pull-abc123");
        assert_eq!(item.repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
        assert_eq!(item.filename, "Qwen3.6-35B-A3B-Q4_K_M.gguf");
        assert_eq!(item.display_name, Some("Qwen3.6 35B".to_string()));
        assert_eq!(item.status, "queued");
        assert_eq!(item.kind, "model");

        // Non-existent job_id should return None
        let none_item = get_item_by_job_id(&conn, "pull-nonexistent").unwrap();
        assert!(none_item.is_none());
    }
}
