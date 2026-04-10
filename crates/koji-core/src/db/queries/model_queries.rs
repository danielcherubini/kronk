//! Model-related database query functions.

use anyhow::Result;
use rusqlite::Connection;

use super::types::{DownloadLogEntry, ModelFileRecord, ModelPullRecord};

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
///
/// **Verification state preservation:** if a row already exists and the incoming
/// `lfs_oid` equals the stored one, the existing `last_verified_at` / `verified_ok`
/// / `verify_error` fields are preserved. If the hash changed (file was re-uploaded
/// on HF) the verification columns are cleared so the file will be re-verified.
pub fn upsert_model_file(
    conn: &Connection,
    repo_id: &str,
    filename: &str,
    quant: Option<&str>,
    lfs_oid: Option<&str>,
    size_bytes: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO model_files
             (repo_id, filename, quant, lfs_oid, size_bytes, downloaded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(repo_id, filename) DO UPDATE SET
             quant         = excluded.quant,
             lfs_oid       = excluded.lfs_oid,
             size_bytes    = excluded.size_bytes,
             downloaded_at = excluded.downloaded_at,
             -- Only clear verification when the hash actually changed.
             last_verified_at = CASE
                 WHEN model_files.lfs_oid IS NOT excluded.lfs_oid THEN NULL
                 ELSE model_files.last_verified_at END,
             verified_ok = CASE
                 WHEN model_files.lfs_oid IS NOT excluded.lfs_oid THEN NULL
                 ELSE model_files.verified_ok END,
             verify_error = CASE
                 WHEN model_files.lfs_oid IS NOT excluded.lfs_oid THEN NULL
                 ELSE model_files.verify_error END",
        (repo_id, filename, quant, lfs_oid, size_bytes),
    )?;
    Ok(())
}

/// Update the verification columns for a single file.
///
/// - `verified_ok = Some(true)`: hash matched; `verify_error` cleared.
/// - `verified_ok = Some(false)`: hash mismatch or verification failure; caller
///   should supply a short `verify_error` message.
/// - `verified_ok = None`: no upstream hash available; `verify_error` optionally
///   set to a reason like `"no upstream hash"`.
///
/// `last_verified_at` is set to the current time via SQLite's `strftime`.
/// The file row must already exist (caller is responsible for ensuring it does
/// via `upsert_model_file` at download time).
pub fn update_verification(
    conn: &Connection,
    repo_id: &str,
    filename: &str,
    verified_ok: Option<bool>,
    verify_error: Option<&str>,
) -> Result<()> {
    let verified_ok_int = verified_ok.map(|b| b as i64);
    // When verified_ok is Some(true), pass NULL for verify_error to clear it
    let verify_error_param = if verified_ok == Some(true) {
        None
    } else {
        verify_error
    };
    let affected = conn.execute(
        "UPDATE model_files SET
              last_verified_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
              verified_ok      = ?3,
              verify_error     = ?4
          WHERE repo_id = ?1 AND filename = ?2",
        (repo_id, filename, verified_ok_int, verify_error_param),
    )?;
    if affected == 0 {
        anyhow::bail!(
            "update_verification: no row found for repo_id={} filename={} \
             (call upsert_model_file first)",
            repo_id,
            filename
        );
    }
    Ok(())
}

/// Get all stored file records for a repo.
pub fn get_model_files(conn: &Connection, repo_id: &str) -> Result<Vec<ModelFileRecord>> {
    let mut stmt = conn.prepare(
        "SELECT repo_id, filename, quant, lfs_oid, size_bytes, downloaded_at,
                last_verified_at, verified_ok, verify_error
         FROM model_files WHERE repo_id = ?1",
    )?;
    let rows = stmt.query_map([repo_id], |row| {
        let verified_ok: Option<i64> = row.get(7)?;
        Ok(ModelFileRecord {
            repo_id: row.get(0)?,
            filename: row.get(1)?,
            quant: row.get(2)?,
            lfs_oid: row.get(3)?,
            size_bytes: row.get(4)?,
            downloaded_at: row.get(5)?,
            last_verified_at: row.get(6)?,
            verified_ok: verified_ok.map(|v| v != 0),
            verify_error: row.get(8)?,
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
