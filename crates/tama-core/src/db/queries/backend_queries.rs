//! Backend installation database query functions.

use anyhow::{Context, Result};
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

// ---------------------------------------------------------------------------
// Backend config queries
// ---------------------------------------------------------------------------

/// A stored config record for a backend.
#[derive(Debug, Clone)]
pub struct BackendConfigRecord {
    pub id: i64,
    pub name: String,
    pub gpu_variant: String,
    /// Parsed from JSON array stored in `default_args` column.
    pub default_args: Vec<String>,
    pub health_check_url: Option<String>,
}

/// Raw row struct for backend_configs before JSON parsing.
#[derive(Debug)]
struct RawBackendConfigRow {
    id: i64,
    name: String,
    gpu_variant: String,
    default_args_raw: Option<String>,
    health_check_url: Option<String>,
}

fn map_raw_backend_config(row: &rusqlite::Row) -> rusqlite::Result<RawBackendConfigRow> {
    Ok(RawBackendConfigRow {
        id: row.get(0)?,
        name: row.get(1)?,
        gpu_variant: row.get(2)?,
        default_args_raw: row.get(3)?,
        health_check_url: row.get(4)?,
    })
}

fn raw_to_record(raw: RawBackendConfigRow) -> Result<BackendConfigRecord> {
    let default_args: Vec<String> = match raw.default_args_raw {
        Some(ref s) if !s.is_empty() => {
            serde_json::from_str(s).context("Failed to parse default_args JSON")?
        }
        _ => Vec::new(),
    };

    Ok(BackendConfigRecord {
        id: raw.id,
        name: raw.name,
        gpu_variant: raw.gpu_variant,
        default_args,
        health_check_url: raw.health_check_url,
    })
}

/// Get the backend config for a given name and gpu_variant.
pub fn get_backend_config(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
) -> Result<Option<BackendConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, gpu_variant, default_args, health_check_url
         FROM backend_configs
         WHERE name = ?1 AND gpu_variant = ?2",
    )?;
    let mut rows = stmt.query_map((name, gpu_variant), map_raw_backend_config)?;
    match rows.next() {
        Some(row) => {
            let raw = row?;
            Ok(Some(raw_to_record(raw)?))
        }
        None => Ok(None),
    }
}

/// Insert or replace a backend config record. Returns the row's id.
pub fn upsert_backend_config(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
    default_args: &[String],
    health_check_url: Option<&str>,
) -> Result<i64> {
    let default_args_json = if default_args.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(default_args)
                .context("Failed to serialize default_args to JSON")?,
        )
    };

    conn.execute(
        "INSERT INTO backend_configs (name, gpu_variant, default_args, health_check_url)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name, gpu_variant) DO UPDATE SET
             default_args = excluded.default_args,
             health_check_url = excluded.health_check_url",
        (
            name,
            gpu_variant,
            default_args_json.as_deref(),
            health_check_url,
        ),
    )?;

    // Fetch the id of the (possibly updated) row
    let id: i64 = conn.query_row(
        "SELECT id FROM backend_configs WHERE name = ?1 AND gpu_variant = ?2",
        (name, gpu_variant),
        |row| row.get(0),
    )?;

    Ok(id)
}

/// Return all backend config records.
pub fn list_backend_configs(conn: &Connection) -> Result<Vec<BackendConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, gpu_variant, default_args, health_check_url
         FROM backend_configs",
    )?;
    let raw_rows = stmt.query_map([], map_raw_backend_config)?;
    let records: Vec<BackendConfigRecord> = raw_rows
        .map(|row| raw_to_record(row?))
        .collect::<Result<Vec<_>>>()?;
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, OpenResult};

    #[test]
    fn test_upsert_backend_config_insert() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let args = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        let id = upsert_backend_config(
            &conn,
            "llama_cpp",
            "cpu",
            &args,
            Some("http://localhost:8080/health"),
        )
        .unwrap();
        assert_eq!(id, 1);

        let record = get_backend_config(&conn, "llama_cpp", "cpu")
            .unwrap()
            .unwrap();
        assert_eq!(record.id, 1);
        assert_eq!(record.name, "llama_cpp");
        assert_eq!(record.gpu_variant, "cpu");
        assert_eq!(record.default_args, args);
        assert_eq!(
            record.health_check_url,
            Some("http://localhost:8080/health".to_string())
        );
    }

    #[test]
    fn test_upsert_backend_config_update() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Insert initial row
        let id1 = upsert_backend_config(
            &conn,
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string()],
            Some("http://localhost:8080/health"),
        )
        .unwrap();

        // Upsert with different values
        let id2 = upsert_backend_config(
            &conn,
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string(), "-b 2048".to_string()],
            Some("http://localhost:9090/health"),
        )
        .unwrap();

        // ID should be the same (updated, not re-inserted)
        assert_eq!(id1, id2);

        let record = get_backend_config(&conn, "llama_cpp", "cpu")
            .unwrap()
            .unwrap();
        assert_eq!(record.default_args, vec!["-fa 1", "-b 2048"]);
        assert_eq!(
            record.health_check_url,
            Some("http://localhost:9090/health".to_string())
        );
    }

    #[test]
    fn test_get_backend_config_not_found() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let result = get_backend_config(&conn, "nonexistent", "cpu").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_backend_configs() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        upsert_backend_config(
            &conn,
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string()],
            Some("http://localhost:8080/health"),
        )
        .unwrap();
        upsert_backend_config(&conn, "llama_cpp", "vulkan", &[], None).unwrap();
        upsert_backend_config(&conn, "ik_llama", "cpu", &[], None).unwrap();

        let configs = list_backend_configs(&conn).unwrap();
        assert_eq!(configs.len(), 3);

        // Verify each config
        let cpu = configs
            .iter()
            .find(|c| c.name == "llama_cpp" && c.gpu_variant == "cpu")
            .unwrap();
        assert_eq!(cpu.default_args, vec!["-fa 1"]);

        let vulkan = configs
            .iter()
            .find(|c| c.name == "llama_cpp" && c.gpu_variant == "vulkan")
            .unwrap();
        assert!(vulkan.default_args.is_empty());
        assert!(vulkan.health_check_url.is_none());
    }

    #[test]
    fn test_upsert_backend_config_empty_args() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let id = upsert_backend_config(&conn, "empty_backend", "cpu", &[], None).unwrap();
        assert_eq!(id, 1);

        let record = get_backend_config(&conn, "empty_backend", "cpu")
            .unwrap()
            .unwrap();
        assert!(record.default_args.is_empty());
        assert!(record.health_check_url.is_none());
    }
}
