//! Last-used model database query functions.
//!
//! Persists the most recently accessed model so the wildcard fallback
//! ("whatevers-hot-n-fresh") can survive proxy restarts.

use anyhow::Result;
use rusqlite::Connection;

use super::types::LastUsedModelRecord;

/// Get the last used model. Returns None if never set.
pub fn get_last_used_model(conn: &Connection) -> Result<Option<LastUsedModelRecord>> {
    let mut stmt =
        conn.prepare("SELECT server_name, model_name, used_at FROM last_used_model WHERE id = 1")?;
    let rows = stmt.query_map([], |row| {
        Ok(LastUsedModelRecord {
            server_name: row.get(0)?,
            model_name: row.get(1)?,
            used_at: row.get(2)?,
        })
    })?;
    let record: Option<LastUsedModelRecord> = rows.into_iter().next().transpose()?;
    Ok(record)
}

/// Set (or replace) the last used model. Single row, id = 1.
pub fn set_last_used_model(conn: &Connection, server_name: &str, model_name: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO last_used_model (id, server_name, model_name, used_at) \
         VALUES (1, ?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        (server_name, model_name),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory SQLite connection with the last_used_model table.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE last_used_model (
                id INTEGER PRIMARY KEY,
                server_name TEXT NOT NULL,
                model_name TEXT NOT NULL,
                used_at TEXT NOT NULL
            )",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_get_last_used_model_empty_table() {
        let conn = test_conn();
        let result = get_last_used_model(&conn).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_set_and_get_last_used_model() {
        let conn = test_conn();
        set_last_used_model(&conn, "my-server", "qwen3.6-35b.gguf").unwrap();

        let record = get_last_used_model(&conn).unwrap();
        assert!(record.is_some());
        let record = record.unwrap();
        assert_eq!(record.server_name, "my-server");
        assert_eq!(record.model_name, "qwen3.6-35b.gguf");
        assert!(!record.used_at.is_empty());
    }

    #[test]
    fn test_set_last_used_model_replaces_existing() {
        let conn = test_conn();
        set_last_used_model(&conn, "server-a", "model-a.gguf").unwrap();
        set_last_used_model(&conn, "server-b", "model-b.gguf").unwrap();

        // Only one row should exist
        let record = get_last_used_model(&conn).unwrap();
        assert!(record.is_some());
        let record = record.unwrap();
        assert_eq!(record.server_name, "server-b");
        assert_eq!(record.model_name, "model-b.gguf");
    }
}
