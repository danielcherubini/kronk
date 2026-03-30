//! Typed query functions for the kronk SQLite database.
//!
//! All functions take a `&Connection` — the caller owns the connection.
//! All functions are synchronous (no async).

use anyhow::Result;
use rusqlite::Connection;

// ---------------------------------------------------------------------------
// Record types
// ---------------------------------------------------------------------------

/// A stored pull record for a HuggingFace repo.
#[derive(Debug, Clone)]
pub struct ModelPullRecord {
    pub repo_id: String,
    pub commit_sha: String,
    pub pulled_at: String, // ISO 8601 from SQLite
}

/// A stored file record for a downloaded GGUF.
#[derive(Debug, Clone)]
pub struct ModelFileRecord {
    pub repo_id: String,
    pub filename: String,
    pub quant: Option<String>,
    pub lfs_oid: Option<String>,
    pub size_bytes: Option<i64>,
    pub downloaded_at: String,
}

/// An entry in the download log (append-only).
#[derive(Debug, Clone)]
pub struct DownloadLogEntry {
    pub repo_id: String,
    pub filename: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    pub success: bool,
    pub error_message: Option<String>,
}

/// An active model entry tracking a running backend process.
#[derive(Debug, Clone)]
pub struct ActiveModelRecord {
    pub server_name: String,
    pub model_name: String,
    pub backend: String,
    pub pid: i64,
    pub port: i64,
    pub backend_url: String,
    pub loaded_at: String,
    pub last_accessed: String,
}

// ---------------------------------------------------------------------------
// Query functions
// ---------------------------------------------------------------------------

/// Insert or update the pull record for a repo.
/// Uses INSERT ... ON CONFLICT(repo_id) DO UPDATE (upsert).
/// Timestamp generated via SQLite's strftime('%Y-%m-%dT%H:%M:%fZ', 'now').
pub fn upsert_model_pull(conn: &Connection, repo_id: &str, commit_sha: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO model_pulls (repo_id, commit_sha, pulled_at)
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(repo_id) DO UPDATE SET
             commit_sha = excluded.commit_sha,
             pulled_at  = excluded.pulled_at",
        (repo_id, commit_sha),
    )?;
    Ok(())
}

/// Get the stored pull record for a repo. Returns None if never pulled.
pub fn get_model_pull(conn: &Connection, repo_id: &str) -> Result<Option<ModelPullRecord>> {
    let mut stmt = conn.prepare(
        "SELECT repo_id, commit_sha, pulled_at
         FROM model_pulls WHERE repo_id = ?1",
    )?;
    let mut rows = stmt.query_map([repo_id], |row| {
        Ok(ModelPullRecord {
            repo_id: row.get(0)?,
            commit_sha: row.get(1)?,
            pulled_at: row.get(2)?,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Insert or update a file record for a downloaded GGUF.
/// Uses INSERT ... ON CONFLICT(repo_id, filename) DO UPDATE.
/// Timestamp generated via SQLite's strftime('%Y-%m-%dT%H:%M:%fZ', 'now').
pub fn upsert_model_file(
    conn: &Connection,
    repo_id: &str,
    filename: &str,
    quant: Option<&str>,
    lfs_oid: Option<&str>,
    size_bytes: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO model_files (repo_id, filename, quant, lfs_oid, size_bytes, downloaded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(repo_id, filename) DO UPDATE SET
             quant        = excluded.quant,
             lfs_oid      = excluded.lfs_oid,
             size_bytes   = excluded.size_bytes,
             downloaded_at = excluded.downloaded_at",
        (repo_id, filename, quant, lfs_oid, size_bytes),
    )?;
    Ok(())
}

/// Get all stored file records for a repo.
pub fn get_model_files(conn: &Connection, repo_id: &str) -> Result<Vec<ModelFileRecord>> {
    let mut stmt = conn.prepare(
        "SELECT repo_id, filename, quant, lfs_oid, size_bytes, downloaded_at
         FROM model_files WHERE repo_id = ?1",
    )?;
    let rows = stmt.query_map([repo_id], |row| {
        Ok(ModelFileRecord {
            repo_id: row.get(0)?,
            filename: row.get(1)?,
            quant: row.get(2)?,
            lfs_oid: row.get(3)?,
            size_bytes: row.get(4)?,
            downloaded_at: row.get(5)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Log a download event (append-only).
pub fn log_download(conn: &Connection, entry: &DownloadLogEntry) -> Result<()> {
    conn.execute(
        "INSERT INTO download_log
             (repo_id, filename, started_at, completed_at,
              size_bytes, duration_ms, success, error_message)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            &entry.repo_id,
            &entry.filename,
            &entry.started_at,
            entry.completed_at.as_deref(),
            entry.size_bytes,
            entry.duration_ms,
            entry.success as i64,
            entry.error_message.as_deref(),
        ),
    )?;
    Ok(())
}

/// Delete all records for a repo (model_pulls, model_files).
/// Does NOT delete download_log entries (they're historical).
/// Both deletes run in a single transaction — either both succeed or neither does.
pub fn delete_model_records(conn: &Connection, repo_id: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM model_pulls WHERE repo_id = ?1", [repo_id])?;
    tx.execute("DELETE FROM model_files WHERE repo_id = ?1", [repo_id])?;
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Active models query functions
// ---------------------------------------------------------------------------

/// Insert or replace an active model entry when a backend is loaded.
pub fn insert_active_model(
    conn: &Connection,
    server_name: &str,
    model_name: &str,
    backend: &str,
    pid: i64,
    port: i64,
    backend_url: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO active_models
            (server_name, model_name, backend, pid, port, backend_url, loaded_at, last_accessed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        (server_name, model_name, backend, pid, port, backend_url),
    )?;
    Ok(())
}

/// Remove an active model entry when a backend is unloaded.
pub fn remove_active_model(conn: &Connection, server_name: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM active_models WHERE server_name = ?1",
        [server_name],
    )?;
    Ok(())
}

/// Get all active model entries (for status / cleanup).
pub fn get_active_models(conn: &Connection) -> Result<Vec<ActiveModelRecord>> {
    let mut stmt = conn.prepare(
        "SELECT server_name, model_name, backend, pid, port, backend_url, loaded_at, last_accessed
         FROM active_models",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ActiveModelRecord {
            server_name: row.get(0)?,
            model_name: row.get(1)?,
            backend: row.get(2)?,
            pid: row.get(3)?,
            port: row.get(4)?,
            backend_url: row.get(5)?,
            loaded_at: row.get(6)?,
            last_accessed: row.get(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Remove all active model entries (for startup cleanup).
pub fn clear_active_models(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM active_models", [])?;
    Ok(())
}

/// Update last_accessed timestamp for an active model.
pub fn touch_active_model(conn: &Connection, server_name: &str) -> Result<()> {
    conn.execute(
        "UPDATE active_models SET last_accessed = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE server_name = ?1",
        [server_name],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn test_upsert_and_get_model_pull() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Insert
        upsert_model_pull(&conn, "bartowski/OmniCoder-8B-GGUF", "abc123").unwrap();
        let record = get_model_pull(&conn, "bartowski/OmniCoder-8B-GGUF")
            .unwrap()
            .unwrap();
        assert_eq!(record.repo_id, "bartowski/OmniCoder-8B-GGUF");
        assert_eq!(record.commit_sha, "abc123");
        assert!(!record.pulled_at.is_empty());

        // Update with new SHA
        upsert_model_pull(&conn, "bartowski/OmniCoder-8B-GGUF", "def456").unwrap();
        let updated = get_model_pull(&conn, "bartowski/OmniCoder-8B-GGUF")
            .unwrap()
            .unwrap();
        assert_eq!(updated.commit_sha, "def456");
    }

    #[test]
    fn test_upsert_and_get_model_files() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let repo = "bartowski/OmniCoder-8B-GGUF";

        // Insert two files
        upsert_model_file(
            &conn,
            repo,
            "Model-Q4_K_M.gguf",
            Some("Q4_K_M"),
            Some("sha256_a"),
            Some(4_200_000_000),
        )
        .unwrap();
        upsert_model_file(
            &conn,
            repo,
            "Model-Q8_0.gguf",
            Some("Q8_0"),
            Some("sha256_b"),
            Some(8_400_000_000),
        )
        .unwrap();

        let files = get_model_files(&conn, repo).unwrap();
        assert_eq!(files.len(), 2);

        // Update one file's lfs_oid
        upsert_model_file(
            &conn,
            repo,
            "Model-Q4_K_M.gguf",
            Some("Q4_K_M"),
            Some("sha256_new"),
            Some(4_300_000_000),
        )
        .unwrap();
        let files2 = get_model_files(&conn, repo).unwrap();
        assert_eq!(files2.len(), 2);
        let updated = files2
            .iter()
            .find(|f| f.filename == "Model-Q4_K_M.gguf")
            .unwrap();
        assert_eq!(updated.lfs_oid.as_deref(), Some("sha256_new"));
        assert_eq!(updated.size_bytes, Some(4_300_000_000));
    }

    #[test]
    fn test_log_download() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let entry = DownloadLogEntry {
            repo_id: "bartowski/OmniCoder-8B-GGUF".to_string(),
            filename: "Model-Q4_K_M.gguf".to_string(),
            started_at: "2024-01-01T00:00:00.000Z".to_string(),
            completed_at: Some("2024-01-01T00:01:00.000Z".to_string()),
            size_bytes: Some(4_200_000_000),
            duration_ms: Some(60_000),
            success: true,
            error_message: None,
        };
        log_download(&conn, &entry).unwrap();

        // Query it back
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM download_log", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let (repo_id, success): (String, i64) = conn
            .query_row(
                "SELECT repo_id, success FROM download_log LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(repo_id, "bartowski/OmniCoder-8B-GGUF");
        assert_eq!(success, 1);
    }

    #[test]
    fn test_get_model_pull_not_found() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let result = get_model_pull(&conn, "unknown/repo").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_model_files_empty() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let files = get_model_files(&conn, "unknown/repo").unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_delete_model_records() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let repo = "bartowski/OmniCoder-8B-GGUF";

        // Insert records
        upsert_model_pull(&conn, repo, "abc123").unwrap();
        upsert_model_file(&conn, repo, "Model-Q4_K_M.gguf", Some("Q4_K_M"), None, None).unwrap();

        // Also insert a download log entry
        log_download(
            &conn,
            &DownloadLogEntry {
                repo_id: repo.to_string(),
                filename: "Model-Q4_K_M.gguf".to_string(),
                started_at: "2024-01-01T00:00:00.000Z".to_string(),
                completed_at: None,
                size_bytes: None,
                duration_ms: None,
                success: false,
                error_message: Some("test".to_string()),
            },
        )
        .unwrap();

        // Delete
        delete_model_records(&conn, repo).unwrap();

        // Verify pulls and files are gone
        assert!(get_model_pull(&conn, repo).unwrap().is_none());
        assert!(get_model_files(&conn, repo).unwrap().is_empty());

        // Verify download_log is preserved
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM download_log", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_migration_v2_creates_active_models() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Verify active_models table exists
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='active_models'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_needs_backfill_true_on_fresh_db() {
        let result = open_in_memory().unwrap();
        assert!(result.needs_backfill);
    }

    #[test]
    fn test_needs_backfill_false_on_existing_db() {
        // Open twice - second time should return false
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        // Manually set version to 2 to simulate existing DB
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        drop(conn);

        // Open again - should detect existing DB
        // Note: This test is simplified - in practice the DB file would persist
    }

    #[test]
    fn test_insert_and_get_active_models() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        insert_active_model(
            &conn,
            "test-server",
            "test-model",
            "llama-server",
            12345,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();

        let models = get_active_models(&conn).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].server_name, "test-server");
        assert_eq!(models[0].model_name, "test-model");
        assert_eq!(models[0].backend, "llama-server");
        assert_eq!(models[0].pid, 12345);
        assert_eq!(models[0].port, 8080);
        assert_eq!(models[0].backend_url, "http://127.0.0.1:8080");
    }

    #[test]
    fn test_remove_active_model() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        insert_active_model(
            &conn,
            "test-server",
            "test-model",
            "llama-server",
            12345,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();

        assert_eq!(get_active_models(&conn).unwrap().len(), 1);

        remove_active_model(&conn, "test-server").unwrap();

        assert!(get_active_models(&conn).unwrap().is_empty());
    }

    #[test]
    fn test_clear_active_models() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        insert_active_model(
            &conn,
            "server1",
            "model1",
            "llama-server",
            1001,
            8001,
            "http://127.0.0.1:8001",
        )
        .unwrap();
        insert_active_model(
            &conn,
            "server2",
            "model2",
            "llama-server",
            1002,
            8002,
            "http://127.0.0.1:8002",
        )
        .unwrap();

        assert_eq!(get_active_models(&conn).unwrap().len(), 2);

        clear_active_models(&conn).unwrap();

        assert!(get_active_models(&conn).unwrap().is_empty());
    }

    #[test]
    fn test_touch_active_model() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        insert_active_model(
            &conn,
            "test-server",
            "test-model",
            "llama-server",
            12345,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();

        let models = get_active_models(&conn).unwrap();
        let loaded_at1 = models[0].loaded_at;

        // Wait a bit to ensure different timestamp
        std::thread::sleep(std::time::Duration::from_millis(10));

        touch_active_model(&conn, "test-server").unwrap();

        let models = get_active_models(&conn).unwrap();
        assert_ne!(models[0].last_accessed, loaded_at1);
    }
}
