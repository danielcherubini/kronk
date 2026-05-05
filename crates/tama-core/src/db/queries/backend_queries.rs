//! Backend installation database query functions.

use anyhow::Result;
use rusqlite::Connection;

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
    pub gpu_variant: String,
    pub source: Option<String>,
    pub is_active: bool,
}

/// Shared row-mapping closure for BackendInstallationRecord queries.
///
/// Extracted to a function so it can be reused across multiple query_map
/// calls without hitting Rust's "each closure has a unique type" issue.
fn map_backend_record(row: &rusqlite::Row) -> rusqlite::Result<BackendInstallationRecord> {
    Ok(BackendInstallationRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        backend_type: row.get(2)?,
        version: row.get(3)?,
        path: row.get(4)?,
        installed_at: row.get(5)?,
        gpu_type: row.get(6)?,
        gpu_variant: row.get(7)?,
        source: row.get(8)?,
        is_active: row.get::<_, i64>(9)? != 0,
    })
}

/// Insert or replace a backend installation record, marking it as active.
///
/// In a single transaction:
/// 1. Inserts (or replaces) the row with `is_active = 1`.
/// 2. Sets `is_active = 0` for all other rows with the same name AND gpu_variant.
///
/// When a row with the same `(name, gpu_variant, version)` already exists, SQLite's `REPLACE`
/// semantics delete the old row and re-insert (the row gets a new `id`). All other rows with
/// the same name and gpu_variant are deactivated (different variants are unaffected).
pub fn insert_backend_installation(
    conn: &Connection,
    record: &BackendInstallationRecord,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO backend_installations
             (name, backend_type, version, path, installed_at, gpu_type, gpu_variant, source, is_active)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
        (
            &record.name,
            &record.backend_type,
            &record.version,
            &record.path,
            record.installed_at,
            record.gpu_type.as_deref(),
            &record.gpu_variant,
            record.source.as_deref(),
        ),
    )?;
    tx.execute(
        "UPDATE backend_installations SET is_active = 0 WHERE name = ?1 AND gpu_variant = ?2 AND version != ?3",
        (&record.name, &record.gpu_variant, &record.version),
    )?;
    tx.commit()?;
    Ok(())
}

/// Get the active backend installation for a given name and gpu_variant.
pub fn get_active_backend(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
) -> Result<Option<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_type, gpu_variant, source, is_active
         FROM backend_installations
         WHERE name = ?1 AND gpu_variant = ?2 AND is_active = 1",
    )?;
    let mut rows = stmt.query_map((name, gpu_variant), map_backend_record)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Return all active backend installations (one per backend name/variant).
pub fn list_active_backends(conn: &Connection) -> Result<Vec<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_type, gpu_variant, source, is_active
         FROM backend_installations
         WHERE is_active = 1",
    )?;
    let rows = stmt.query_map([], map_backend_record)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Return all versions of a backend, ordered by `installed_at DESC` (newest first).
///
/// If `gpu_variant` is `Some`, only returns rows matching that variant.
/// If `None`, returns all variants.
pub fn list_backend_versions(
    conn: &Connection,
    name: &str,
    gpu_variant: Option<&str>,
) -> Result<Vec<BackendInstallationRecord>> {
    let sql = if let Some(_variant) = gpu_variant {
        "SELECT id, name, backend_type, version, path, installed_at, gpu_type, gpu_variant, source, is_active
         FROM backend_installations
         WHERE name = ?1 AND gpu_variant = ?2
         ORDER BY installed_at DESC"
    } else {
        "SELECT id, name, backend_type, version, path, installed_at, gpu_type, gpu_variant, source, is_active
         FROM backend_installations
         WHERE name = ?1
         ORDER BY installed_at DESC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = if let Some(variant) = gpu_variant {
        stmt.query_map((name, variant), map_backend_record)?
    } else {
        stmt.query_map([name], map_backend_record)?
    };
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Get a specific backend installation by (name, gpu_variant, version).
/// Returns Ok(None) if no row matches.
pub fn get_backend_by_version(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<Option<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_type, gpu_variant, source, is_active
         FROM backend_installations
         WHERE name = ?1 AND gpu_variant = ?2 AND version = ?3",
    )?;
    let mut rows = stmt.query_map((name, gpu_variant, version), map_backend_record)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Delete a specific `(name, gpu_variant, version)` backend installation row.
pub fn delete_backend_installation(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM backend_installations WHERE name = ?1 AND gpu_variant = ?2 AND version = ?3",
        (name, gpu_variant, version),
    )?;
    Ok(())
}

/// Deactivate all versions for a backend name+variant, then activate the specified version.
///
/// This is an atomic operation executed in a transaction:
/// 1. Check if the target version exists
/// 2. If not, return Ok(false) without any changes
/// 3. SET is_active = 0 for all rows with the given name AND gpu_variant
/// 4. SET is_active = 1 for the row matching (name, gpu_variant, version)
///
/// Returns Ok(true) if the version was found and activated, Ok(false) if no matching row exists.
pub fn activate_backend_version(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;

    // Check if the target version exists before making any changes
    let exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM backend_installations WHERE name = ?1 AND gpu_variant = ?2 AND version = ?3",
        (name, gpu_variant, version),
        |row| row.get(0),
    )?;

    if exists == 0 {
        tx.commit()?;
        return Ok(false);
    }

    // Deactivate all versions for this backend+variant
    tx.execute(
        "UPDATE backend_installations SET is_active = 0 WHERE name = ?1 AND gpu_variant = ?2",
        (name, gpu_variant),
    )?;

    // Activate the requested version
    let changes = tx.execute(
        "UPDATE backend_installations SET is_active = 1 WHERE name = ?1 AND gpu_variant = ?2 AND version = ?3",
        (name, gpu_variant, version),
    )?;

    tx.commit()?;
    Ok(changes > 0)
}

/// Delete all installation rows for a backend name (used by `backend remove`).
///
/// If `gpu_variant` is `Some`, only deletes rows matching that variant.
/// If `None`, deletes all variants.
pub fn delete_all_backend_versions(
    conn: &Connection,
    name: &str,
    gpu_variant: Option<&str>,
) -> Result<()> {
    if let Some(variant) = gpu_variant {
        conn.execute(
            "DELETE FROM backend_installations WHERE name = ?1 AND gpu_variant = ?2",
            (name, variant),
        )?;
    } else {
        conn.execute("DELETE FROM backend_installations WHERE name = ?1", [name])?;
    }
    Ok(())
}
