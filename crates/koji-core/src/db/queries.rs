//! Typed query functions for the koji SQLite database.
//!
//! All functions take a `&Connection` — the caller owns the connection.
//! All functions are synchronous (no async).

use anyhow::{bail, Result};
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
    /// ISO 8601 timestamp of the most recent verification attempt. None if never verified.
    pub last_verified_at: Option<String>,
    /// Some(true) = hash matched. Some(false) = mismatch. None = never verified
    /// or no upstream hash available to compare against.
    pub verified_ok: Option<bool>,
    /// Short human-readable error when `verified_ok = Some(false)` or when verification
    /// could not complete (e.g. "no upstream hash", "hash mismatch: expected X got Y").
    pub verify_error: Option<String>,
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
    let affected = conn.execute(
        "UPDATE model_files SET
             last_verified_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
             verified_ok      = ?3,
             verify_error     = ?4
         WHERE repo_id = ?1 AND filename = ?2",
        (repo_id, filename, verified_ok_int, verify_error),
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

/// Rename an active model by updating its primary key (server_name).
pub fn rename_active_model(conn: &Connection, old_name: &str, new_name: &str) -> Result<()> {
    conn.execute(
        "UPDATE active_models SET server_name = ?2 WHERE server_name = ?1",
        [old_name, new_name],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Backend installations record type and query functions
// ---------------------------------------------------------------------------

/// A stored installation record for a backend binary.
#[derive(Debug, Clone)]
pub struct BackendInstallationRecord {
    /// Set to 0 when constructing a record for INSERT (DB assigns the real id via AUTOINCREMENT).
    pub id: i64,
    pub name: String,
    pub backend_type: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    pub gpu_type: Option<String>,
    pub source: Option<String>,
    pub is_active: bool,
}

/// Insert or replace a backend installation record, marking it as active.
///
/// In a single transaction:
/// 1. Inserts (or replaces) the row with `is_active = 1`.
/// 2. Sets `is_active = 0` for all other rows with the same name.
///
/// When a row with the same `(name, version)` already exists, SQLite's `REPLACE` semantics
/// delete the old row and re-insert (the row gets a new `id`). All other rows with the same
/// name are deactivated.
pub fn insert_backend_installation(
    conn: &Connection,
    record: &BackendInstallationRecord,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO backend_installations
             (name, backend_type, version, path, installed_at, gpu_type, source, is_active)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
        (
            &record.name,
            &record.backend_type,
            &record.version,
            &record.path,
            record.installed_at,
            record.gpu_type.as_deref(),
            record.source.as_deref(),
        ),
    )?;
    tx.execute(
        "UPDATE backend_installations SET is_active = 0 WHERE name = ?1 AND version != ?2",
        (&record.name, &record.version),
    )?;
    tx.commit()?;
    Ok(())
}

/// Get the active backend installation for a given name.
pub fn get_active_backend(
    conn: &Connection,
    name: &str,
) -> Result<Option<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_type, source, is_active
         FROM backend_installations
         WHERE name = ?1 AND is_active = 1",
    )?;
    let mut rows = stmt.query_map([name], |row| {
        Ok(BackendInstallationRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            backend_type: row.get(2)?,
            version: row.get(3)?,
            path: row.get(4)?,
            installed_at: row.get(5)?,
            gpu_type: row.get(6)?,
            source: row.get(7)?,
            is_active: row.get::<_, i64>(8)? != 0,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Return all active backend installations (one per backend name).
pub fn list_active_backends(conn: &Connection) -> Result<Vec<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_type, source, is_active
         FROM backend_installations
         WHERE is_active = 1",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(BackendInstallationRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            backend_type: row.get(2)?,
            version: row.get(3)?,
            path: row.get(4)?,
            installed_at: row.get(5)?,
            gpu_type: row.get(6)?,
            source: row.get(7)?,
            is_active: row.get::<_, i64>(8)? != 0,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Return all versions of a backend, ordered by `installed_at DESC` (newest first).
pub fn list_backend_versions(
    conn: &Connection,
    name: &str,
) -> Result<Vec<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_type, source, is_active
         FROM backend_installations
         WHERE name = ?1
         ORDER BY installed_at DESC",
    )?;
    let rows = stmt.query_map([name], |row| {
        Ok(BackendInstallationRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            backend_type: row.get(2)?,
            version: row.get(3)?,
            path: row.get(4)?,
            installed_at: row.get(5)?,
            gpu_type: row.get(6)?,
            source: row.get(7)?,
            is_active: row.get::<_, i64>(8)? != 0,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Get a specific backend installation by (name, version).
/// Returns Ok(None) if no row matches.
pub fn get_backend_by_version(
    conn: &Connection,
    name: &str,
    version: &str,
) -> Result<Option<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_type, source, is_active
         FROM backend_installations
         WHERE name = ?1 AND version = ?2",
    )?;
    let mut rows = stmt.query_map((name, version), |row| {
        Ok(BackendInstallationRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            backend_type: row.get(2)?,
            version: row.get(3)?,
            path: row.get(4)?,
            installed_at: row.get(5)?,
            gpu_type: row.get(6)?,
            source: row.get(7)?,
            is_active: row.get::<_, i64>(8)? != 0,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Delete a specific `(name, version)` backend installation row.
pub fn delete_backend_installation(conn: &Connection, name: &str, version: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM backend_installations WHERE name = ?1 AND version = ?2",
        (name, version),
    )?;
    Ok(())
}

/// Delete all installation rows for a backend name (used by `backend remove`).
pub fn delete_all_backend_versions(conn: &Connection, name: &str) -> Result<()> {
    conn.execute("DELETE FROM backend_installations WHERE name = ?1", [name])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// System metrics history record type and query functions
// ---------------------------------------------------------------------------

/// One sample of system-level metrics, persisted in `system_metrics_history`.
#[derive(Debug, Clone)]
pub struct SystemMetricsRow {
    pub ts_unix_ms: i64,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: i64,
    pub ram_total_mib: i64,
    pub gpu_utilization_pct: Option<i64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub models_loaded: i64,
}

/// Insert one sample and prune anything older than `cutoff_ms` in a single
/// transaction. Both operations succeed or fail together so a crash never
/// leaves the table half-pruned.
pub fn insert_system_metric(
    conn: &Connection,
    row: &SystemMetricsRow,
    cutoff_ms: i64,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO system_metrics_history
             (ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
              gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            row.ts_unix_ms,
            row.cpu_usage_pct as f64,
            row.ram_used_mib,
            row.ram_total_mib,
            row.gpu_utilization_pct,
            row.vram_used_mib,
            row.vram_total_mib,
            row.models_loaded,
        ),
    )?;
    tx.execute(
        "DELETE FROM system_metrics_history WHERE ts_unix_ms < ?1",
        [cutoff_ms],
    )?;
    tx.commit()?;
    Ok(())
}

/// Fetch all samples newer than `since_ms` (exclusive), oldest-first.
pub fn get_system_metrics_since(conn: &Connection, since_ms: i64) -> Result<Vec<SystemMetricsRow>> {
    let mut stmt = conn.prepare(
        "SELECT ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
                 gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded
          FROM system_metrics_history
          WHERE ts_unix_ms > ?1
          ORDER BY ts_unix_ms ASC",
    )?;
    let rows = stmt.query_map([since_ms], |row| {
        Ok(SystemMetricsRow {
            ts_unix_ms: row.get(0)?,
            cpu_usage_pct: row.get(1)?,
            ram_used_mib: row.get(2)?,
            ram_total_mib: row.get(3)?,
            gpu_utilization_pct: row.get(4)?,
            vram_used_mib: row.get(5)?,
            vram_total_mib: row.get(6)?,
            models_loaded: row.get(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Fetch the most recent `limit` samples, oldest-first.
pub fn get_recent_system_metrics(conn: &Connection, limit: i64) -> Result<Vec<SystemMetricsRow>> {
    if limit < 0 {
        bail!("limit must be >= 0");
    }
    let mut stmt = conn.prepare(
        "SELECT ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
                 gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded
          FROM system_metrics_history
          ORDER BY ts_unix_ms DESC
          LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(SystemMetricsRow {
            ts_unix_ms: row.get(0)?,
            cpu_usage_pct: row.get(1)?,
            ram_used_mib: row.get(2)?,
            ram_total_mib: row.get(3)?,
            gpu_utilization_pct: row.get(4)?,
            vram_used_mib: row.get(5)?,
            vram_total_mib: row.get(6)?,
            models_loaded: row.get(7)?,
        })
    })?;
    let mut rows: Vec<SystemMetricsRow> = rows.collect::<rusqlite::Result<_>>()?;
    rows.reverse(); // reverse to return oldest-first
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, OpenResult};

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
    fn test_upsert_preserves_verification_when_hash_unchanged() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let repo = "test/repo";

        // Initial insert with a hash
        upsert_model_file(
            &conn,
            repo,
            "Model-Q4_K_M.gguf",
            Some("Q4_K_M"),
            Some("sha_abc"),
            Some(1000),
        )
        .unwrap();

        // Mark it as verified
        update_verification(&conn, repo, "Model-Q4_K_M.gguf", Some(true), None).unwrap();

        // Re-upsert with the SAME hash (e.g. refresh_metadata from HF)
        upsert_model_file(
            &conn,
            repo,
            "Model-Q4_K_M.gguf",
            Some("Q4_K_M"),
            Some("sha_abc"),
            Some(1000),
        )
        .unwrap();

        let files = get_model_files(&conn, repo).unwrap();
        let file = files
            .iter()
            .find(|f| f.filename == "Model-Q4_K_M.gguf")
            .unwrap();
        assert_eq!(
            file.verified_ok,
            Some(true),
            "verification state should be preserved when lfs_oid is unchanged"
        );
        assert!(file.last_verified_at.is_some());
    }

    #[test]
    fn test_upsert_clears_verification_when_hash_changes() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let repo = "test/repo";

        upsert_model_file(
            &conn,
            repo,
            "Model-Q4_K_M.gguf",
            Some("Q4_K_M"),
            Some("sha_old"),
            Some(1000),
        )
        .unwrap();
        update_verification(&conn, repo, "Model-Q4_K_M.gguf", Some(true), None).unwrap();

        // Re-upsert with a DIFFERENT hash (file was updated on HF)
        upsert_model_file(
            &conn,
            repo,
            "Model-Q4_K_M.gguf",
            Some("Q4_K_M"),
            Some("sha_new"),
            Some(1100),
        )
        .unwrap();

        let files = get_model_files(&conn, repo).unwrap();
        let file = files
            .iter()
            .find(|f| f.filename == "Model-Q4_K_M.gguf")
            .unwrap();
        assert_eq!(
            file.verified_ok, None,
            "verification state should be cleared when lfs_oid changes"
        );
        assert!(file.last_verified_at.is_none());
        assert!(file.verify_error.is_none());
    }

    #[test]
    fn test_update_verification_writes_error() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let repo = "test/repo";

        upsert_model_file(&conn, repo, "x.gguf", None, Some("sha"), Some(1)).unwrap();
        update_verification(
            &conn,
            repo,
            "x.gguf",
            Some(false),
            Some("hash mismatch: expected ab got cd"),
        )
        .unwrap();

        let files = get_model_files(&conn, repo).unwrap();
        let f = &files[0];
        assert_eq!(f.verified_ok, Some(false));
        assert_eq!(
            f.verify_error.as_deref(),
            Some("hash mismatch: expected ab got cd")
        );
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
        // Use a real file so the DB persists between two opens
        let tmp = tempfile::tempdir().unwrap();
        let first = crate::db::open(tmp.path()).unwrap();
        assert!(first.needs_backfill, "first open should need backfill");
        drop(first.conn);

        let second = crate::db::open(tmp.path()).unwrap();
        assert!(
            !second.needs_backfill,
            "second open should not need backfill"
        );
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
        let loaded_at1 = models[0].loaded_at.clone();

        // Wait a bit to ensure different timestamp
        std::thread::sleep(std::time::Duration::from_millis(200));

        touch_active_model(&conn, "test-server").unwrap();

        let models = get_active_models(&conn).unwrap();
        assert_ne!(models[0].last_accessed, loaded_at1);
    }

    #[test]
    fn test_rename_active_model() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Insert an active model with old name
        insert_active_model(
            &conn,
            "old-name",
            "test-model",
            "llama-server",
            12345,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();

        // Rename
        rename_active_model(&conn, "old-name", "new-name").unwrap();

        // Verify old name is gone and new name exists
        let models = get_active_models(&conn).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].server_name, "new-name");

        // Verify old name is gone
        let old_model = conn
            .query_row(
                "SELECT COUNT(*) FROM active_models WHERE server_name = ?",
                ["old-name"],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(old_model, 0);
    }

    #[test]
    fn test_rename_active_model_not_found() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Rename a name that doesn't exist — should succeed (0 rows affected is OK)
        let result = rename_active_model(&conn, "non-existent", "new-name");
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // backend_installations tests
    // -----------------------------------------------------------------------

    fn make_record(name: &str, version: &str, installed_at: i64) -> BackendInstallationRecord {
        BackendInstallationRecord {
            id: 0,
            name: name.to_string(),
            backend_type: "llama_cpp".to_string(),
            version: version.to_string(),
            path: format!("/opt/backends/{name}/{version}"),
            installed_at,
            gpu_type: None,
            source: None,
            is_active: false,
        }
    }

    #[test]
    fn test_insert_and_get_active_backend() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let r1 = make_record("llama_cpp", "v1.0.0", 1000);
        insert_backend_installation(&conn, &r1).unwrap();

        let r2 = make_record("llama_cpp", "v2.0.0", 2000);
        insert_backend_installation(&conn, &r2).unwrap();

        let active = get_active_backend(&conn, "llama_cpp").unwrap().unwrap();
        assert_eq!(active.version, "v2.0.0");
        assert!(active.is_active);
    }

    #[test]
    fn test_list_active_backends() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let r1 = make_record("llama_cpp", "v1.0.0", 1000);
        insert_backend_installation(&conn, &r1).unwrap();

        let r2 = make_record("ik_llama", "v1.0.0", 1000);
        insert_backend_installation(&conn, &r2).unwrap();

        let active = list_active_backends(&conn).unwrap();
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn test_list_backend_versions() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let r1 = make_record("llama_cpp", "v1.0.0", 1000);
        insert_backend_installation(&conn, &r1).unwrap();

        let r2 = make_record("llama_cpp", "v2.0.0", 2000);
        insert_backend_installation(&conn, &r2).unwrap();

        let versions = list_backend_versions(&conn, "llama_cpp").unwrap();
        assert_eq!(versions.len(), 2);
        // Ordered newest first (installed_at DESC)
        assert_eq!(versions[0].version, "v2.0.0");
        assert_eq!(versions[1].version, "v1.0.0");
    }

    #[test]
    fn test_delete_single_backend_version() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let r1 = make_record("llama_cpp", "v1.0.0", 1000);
        insert_backend_installation(&conn, &r1).unwrap();

        let r2 = make_record("llama_cpp", "v2.0.0", 2000);
        insert_backend_installation(&conn, &r2).unwrap();

        delete_backend_installation(&conn, "llama_cpp", "v1.0.0").unwrap();

        let versions = list_backend_versions(&conn, "llama_cpp").unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, "v2.0.0");
    }

    #[test]
    fn test_delete_all_backend_versions() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let r1 = make_record("llama_cpp", "v1.0.0", 1000);
        insert_backend_installation(&conn, &r1).unwrap();

        let r2 = make_record("llama_cpp", "v2.0.0", 2000);
        insert_backend_installation(&conn, &r2).unwrap();

        delete_all_backend_versions(&conn, "llama_cpp").unwrap();

        let versions = list_backend_versions(&conn, "llama_cpp").unwrap();
        assert!(versions.is_empty());
    }

    #[test]
    fn test_get_backend_by_version() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Insert two versions of llama_cpp with distinct paths
        let r1 = BackendInstallationRecord {
            id: 0,
            name: "llama_cpp".to_string(),
            backend_type: "llama_cpp".to_string(),
            version: "v1.0.0".to_string(),
            path: "/v1/llama-server".to_string(),
            installed_at: 1000,
            gpu_type: None,
            source: None,
            is_active: false,
        };
        insert_backend_installation(&conn, &r1).unwrap();

        let r2 = BackendInstallationRecord {
            id: 0,
            name: "llama_cpp".to_string(),
            backend_type: "llama_cpp".to_string(),
            version: "v2.0.0".to_string(),
            path: "/v2/llama-server".to_string(),
            installed_at: 2000,
            gpu_type: None,
            source: None,
            is_active: false,
        };
        insert_backend_installation(&conn, &r2).unwrap();

        // v1.0.0 should be found with path /v1/llama-server
        let found = get_backend_by_version(&conn, "llama_cpp", "v1.0.0")
            .unwrap()
            .unwrap();
        assert_eq!(found.path, "/v1/llama-server");
        assert_eq!(found.version, "v1.0.0");

        // v2.0.0 should be found with path /v2/llama-server
        let found = get_backend_by_version(&conn, "llama_cpp", "v2.0.0")
            .unwrap()
            .unwrap();
        assert_eq!(found.path, "/v2/llama-server");
        assert_eq!(found.version, "v2.0.0");

        // unknown version should return Ok(None)
        let not_found = get_backend_by_version(&conn, "llama_cpp", "unknown").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_insert_same_version_is_idempotent() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let r1 = make_record("llama_cpp", "b8407", 1000);
        insert_backend_installation(&conn, &r1).unwrap();

        // Same (name, version) — should succeed (INSERT OR REPLACE), not error
        let r2 = make_record("llama_cpp", "b8407", 2000);
        let result = insert_backend_installation(&conn, &r2);
        assert!(
            result.is_ok(),
            "reinstalling the same (name, version) should be idempotent, got: {:?}",
            result
        );

        // Only one row should exist for this (name, version)
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM backend_installations WHERE name = 'llama_cpp' AND version = 'b8407'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "only one row should exist after idempotent insert"
        );
    }

    // -----------------------------------------------------------------------
    // system_metrics_history tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_insert_and_get_recent_system_metrics() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let row1 = SystemMetricsRow {
            ts_unix_ms: 1_000_000_000_000,
            cpu_usage_pct: 25.5,
            ram_used_mib: 2048,
            ram_total_mib: 8192,
            gpu_utilization_pct: Some(45),
            vram_used_mib: Some(1024),
            vram_total_mib: Some(8192),
            models_loaded: 3,
        };
        insert_system_metric(&conn, &row1, 0).unwrap();

        let row2 = SystemMetricsRow {
            ts_unix_ms: 2_000_000_000_000,
            cpu_usage_pct: 50.0,
            ram_used_mib: 4096,
            ram_total_mib: 8192,
            gpu_utilization_pct: Some(70),
            vram_used_mib: Some(4096),
            vram_total_mib: Some(8192),
            models_loaded: 5,
        };
        insert_system_metric(&conn, &row2, 0).unwrap();

        let recent = get_recent_system_metrics(&conn, 10).unwrap();
        assert_eq!(recent.len(), 2);
        // Should return oldest-first
        assert_eq!(recent[0].ts_unix_ms, 1_000_000_000_000);
        assert_eq!(recent[1].ts_unix_ms, 2_000_000_000_000);
    }

    #[test]
    fn test_insert_system_metric_prunes_old_rows() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let old_row = SystemMetricsRow {
            ts_unix_ms: 1_000_000_000_000,
            cpu_usage_pct: 10.0,
            ram_used_mib: 1024,
            ram_total_mib: 8192,
            gpu_utilization_pct: None,
            vram_used_mib: None,
            vram_total_mib: None,
            models_loaded: 0,
        };
        insert_system_metric(&conn, &old_row, 5_000_000_000_000).unwrap();

        let new_row = SystemMetricsRow {
            ts_unix_ms: 6_000_000_000_000,
            cpu_usage_pct: 30.0,
            ram_used_mib: 3072,
            ram_total_mib: 8192,
            gpu_utilization_pct: Some(25),
            vram_used_mib: Some(512),
            vram_total_mib: Some(8192),
            models_loaded: 1,
        };
        insert_system_metric(&conn, &new_row, 5_000_000_000_000).unwrap();

        // old_row should have been pruned (ts_unix_ms < cutoff_ms)
        let recent = get_recent_system_metrics(&conn, 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].ts_unix_ms, 6_000_000_000_000);
    }

    #[test]
    fn test_get_system_metrics_since() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let row1 = SystemMetricsRow {
            ts_unix_ms: 1_000_000_000_000,
            cpu_usage_pct: 20.0,
            ram_used_mib: 2048,
            ram_total_mib: 8192,
            gpu_utilization_pct: None,
            vram_used_mib: None,
            vram_total_mib: None,
            models_loaded: 1,
        };
        insert_system_metric(&conn, &row1, 0).unwrap();

        let row2 = SystemMetricsRow {
            ts_unix_ms: 3_000_000_000_000,
            cpu_usage_pct: 40.0,
            ram_used_mib: 4096,
            ram_total_mib: 8192,
            gpu_utilization_pct: Some(60),
            vram_used_mib: Some(2048),
            vram_total_mib: Some(8192),
            models_loaded: 2,
        };
        insert_system_metric(&conn, &row2, 0).unwrap();

        // Get metrics since 2_000_000_000_000 (exclusive)
        let since = get_system_metrics_since(&conn, 2_000_000_000_000).unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].ts_unix_ms, 3_000_000_000_000);
    }
}
