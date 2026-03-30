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
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn test_upsert_and_get_model_pull() {
        let conn = open_in_memory().unwrap();

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
        let conn = open_in_memory().unwrap();
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
        let conn = open_in_memory().unwrap();

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
        let conn = open_in_memory().unwrap();
        let result = get_model_pull(&conn, "unknown/repo").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_model_files_empty() {
        let conn = open_in_memory().unwrap();
        let files = get_model_files(&conn, "unknown/repo").unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_delete_model_records() {
        let conn = open_in_memory().unwrap();
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
}
