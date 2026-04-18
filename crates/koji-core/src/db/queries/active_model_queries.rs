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

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory SQLite connection with the active_models table.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE active_models (
                server_name TEXT PRIMARY KEY,
                model_name TEXT NOT NULL,
                backend TEXT NOT NULL,
                pid INTEGER NOT NULL,
                port INTEGER NOT NULL,
                backend_url TEXT NOT NULL,
                loaded_at TEXT NOT NULL,
                last_accessed TEXT NOT NULL
            )",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_insert_active_model() {
        let conn = test_conn();
        insert_active_model(
            &conn,
            "server1",
            "model.gguf",
            "llama-cpp",
            1234,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();

        let models = get_active_models(&conn).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].server_name, "server1");
        assert_eq!(models[0].model_name, "model.gguf");
        assert_eq!(models[0].pid, 1234);
    }

    #[test]
    fn test_insert_active_model_replaces_existing() {
        let conn = test_conn();
        insert_active_model(
            &conn,
            "server1",
            "model.gguf",
            "llama-cpp",
            1234,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();
        // Insert again with different values — should replace
        insert_active_model(
            &conn,
            "server1",
            "model-v2.gguf",
            "llama-cpp",
            5678,
            8081,
            "http://127.0.0.1:8081",
        )
        .unwrap();

        let models = get_active_models(&conn).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model_name, "model-v2.gguf");
        assert_eq!(models[0].pid, 5678);
    }

    #[test]
    fn test_remove_active_model() {
        let conn = test_conn();
        insert_active_model(
            &conn,
            "server1",
            "model.gguf",
            "llama-cpp",
            1234,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();
        remove_active_model(&conn, "server1").unwrap();

        let models = get_active_models(&conn).unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_remove_active_model_nonexistent() {
        let conn = test_conn();
        // Should not error even if server doesn't exist
        remove_active_model(&conn, "nonexistent").unwrap();
    }

    #[test]
    fn test_get_active_models_empty() {
        let conn = test_conn();
        let models = get_active_models(&conn).unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_get_active_models_multiple() {
        let conn = test_conn();
        insert_active_model(
            &conn,
            "server1",
            "model1.gguf",
            "llama-cpp",
            100,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();
        insert_active_model(
            &conn,
            "server2",
            "model2.gguf",
            "vllm",
            200,
            8081,
            "http://127.0.0.1:8081",
        )
        .unwrap();

        let models = get_active_models(&conn).unwrap();
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn test_clear_active_models() {
        let conn = test_conn();
        insert_active_model(
            &conn,
            "s1",
            "m1.gguf",
            "llama-cpp",
            100,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();
        insert_active_model(
            &conn,
            "s2",
            "m2.gguf",
            "vllm",
            200,
            8081,
            "http://127.0.0.1:8081",
        )
        .unwrap();

        clear_active_models(&conn).unwrap();
        assert!(get_active_models(&conn).unwrap().is_empty());
    }

    #[test]
    fn test_touch_active_model() {
        let conn = test_conn();
        insert_active_model(
            &conn,
            "server1",
            "model.gguf",
            "llama-cpp",
            1234,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();

        // Touch should succeed without error
        touch_active_model(&conn, "server1").unwrap();

        // Verify the record still exists
        let models = get_active_models(&conn).unwrap();
        assert_eq!(models.len(), 1);
    }

    #[test]
    fn test_touch_active_model_nonexistent() {
        let conn = test_conn();
        // Should not error even if server doesn't exist
        touch_active_model(&conn, "nonexistent").unwrap();
    }

    #[test]
    fn test_rename_active_model() {
        let conn = test_conn();
        insert_active_model(
            &conn,
            "old-name",
            "model.gguf",
            "llama-cpp",
            1234,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();

        rename_active_model(&conn, "old-name", "new-name").unwrap();

        let models = get_active_models(&conn).unwrap();
        assert_eq!(models[0].server_name, "new-name");
    }

    #[test]
    fn test_rename_active_model_nonexistent() {
        let conn = test_conn();
        // Should not error even if old name doesn't exist
        rename_active_model(&conn, "old", "new").unwrap();
    }
}
