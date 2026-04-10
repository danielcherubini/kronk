//! Active model database query functions.

use anyhow::Result;
use rusqlite::Connection;

use super::types::ActiveModelRecord;

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
