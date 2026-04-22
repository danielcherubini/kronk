//! TTS configuration database query functions.

use anyhow::Result;
use rusqlite::{params, Connection};

use super::types::TtsConfigRecord;

/// Insert or update the TTS engine configuration.
/// Timestamp updated via SQLite's strftime('%Y-%m-%dT%H:%M:%fZ', 'now') on conflict.
/// Returns the config id.
pub fn upsert_tts_config(conn: &Connection, record: &TtsConfigRecord) -> Result<i64> {
    conn.execute(
        "INSERT INTO tts_configs (
            engine, default_voice, speed, format, enabled,
            created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7
        )
         ON CONFLICT(engine) DO UPDATE SET
             default_voice = excluded.default_voice,
             speed = excluded.speed,
             format = excluded.format,
             enabled = excluded.enabled,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            record.engine,
            record.default_voice,
            record.speed,
            record.format,
            record.enabled as i32,
            record.created_at,
            record.updated_at,
        ],
    )?;
    // Return the id (either existing or newly created)
    let id: i64 = conn.query_row(
        "SELECT id FROM tts_configs WHERE engine = ?1",
        [&record.engine],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Get the TTS configuration by engine name. Returns None if not found.
pub fn get_tts_config(conn: &Connection, engine: &str) -> Result<Option<TtsConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, engine, default_voice, speed, format, enabled,
                created_at, updated_at
         FROM tts_configs WHERE engine = ?1",
    )?;
    let mut rows = stmt.query_map([engine], |row| {
        Ok(TtsConfigRecord {
            id: row.get(0)?,
            engine: row.get(1)?,
            default_voice: row.get(2)?,
            speed: row.get(3)?,
            format: row.get(4)?,
            enabled: row.get::<_, i32>(5)? != 0,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Get all stored TTS engine configurations.
pub fn get_all_tts_configs(conn: &Connection) -> Result<Vec<TtsConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, engine, default_voice, speed, format, enabled,
                created_at, updated_at
         FROM tts_configs ORDER BY engine ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(TtsConfigRecord {
            id: row.get(0)?,
            engine: row.get(1)?,
            default_voice: row.get(2)?,
            speed: row.get(3)?,
            format: row.get(4)?,
            enabled: row.get::<_, i32>(5)? != 0,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Delete the TTS configuration by engine name.
pub fn delete_tts_config(conn: &Connection, engine: &str) -> Result<()> {
    conn.execute("DELETE FROM tts_configs WHERE engine = ?1", [engine])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Create the tts_configs table
        conn.execute_batch(
            r#"
            CREATE TABLE tts_configs (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                engine       TEXT NOT NULL UNIQUE COLLATE NOCASE,
                default_voice TEXT,
                speed        REAL   NOT NULL DEFAULT 1.0,
                format       TEXT   NOT NULL DEFAULT 'mp3',
                enabled      INTEGER NOT NULL DEFAULT 1,
                created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );
            "#,
        )
        .unwrap();
        conn
    }

    /// Test that upsert_tts_config creates a new record and returns its id.
    #[test]
    fn test_upsert_creates_new_record() {
        let conn = setup_test_db();
        let record = TtsConfigRecord {
            id: 0,
            engine: "kokoro".to_string(),
            default_voice: Some("af_sky".to_string()),
            speed: 1.2,
            format: "mp3".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
        };

        let id = upsert_tts_config(&conn, &record).unwrap();
        assert_eq!(id, 1);
    }

    /// Test that get_tts_config returns the correct record.
    #[test]
    fn test_get_tts_config_returns_record() {
        let conn = setup_test_db();
        let record = TtsConfigRecord {
            id: 0,
            engine: "kokoro".to_string(),
            default_voice: Some("af_nicole".to_string()),
            speed: 1.5,
            format: "wav".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        upsert_tts_config(&conn, &record).unwrap();

        let found = get_tts_config(&conn, "kokoro").unwrap().unwrap();
        assert_eq!(found.engine, "kokoro");
        assert_eq!(found.speed, 1.5);
        assert_eq!(found.format, "wav");
    }

    /// Test that get_tts_config returns None for unknown engine.
    #[test]
    fn test_get_tts_config_returns_none_for_unknown() {
        let conn = setup_test_db();
        let result = get_tts_config(&conn, "unknown_engine").unwrap();
        assert!(result.is_none());
    }

    /// Test that engine lookup is case-insensitive (COLLATE NOCASE).
    #[test]
    fn test_case_insensitive_engine_lookup() {
        let conn = setup_test_db();
        let record = TtsConfigRecord {
            id: 0,
            engine: "Kokoro".to_string(), // Capital K
            default_voice: None,
            speed: 1.0,
            format: "mp3".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        upsert_tts_config(&conn, &record).unwrap();

        // Lookup with lowercase should find it
        let found = get_tts_config(&conn, "kokoro").unwrap().unwrap();
        assert_eq!(found.engine, "Kokoro");

        // Lookup with mixed case should also find it
        let found2 = get_tts_config(&conn, "KOKORO").unwrap().unwrap();
        assert_eq!(found2.engine, "Kokoro");
    }

    /// Test that upsert updates an existing record.
    #[test]
    fn test_upsert_updates_existing_record() {
        let conn = setup_test_db();

        // Insert initial config
        let record1 = TtsConfigRecord {
            id: 0,
            engine: "kokoro".to_string(),
            default_voice: Some("af_sky".to_string()),
            speed: 1.0,
            format: "mp3".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let id1 = upsert_tts_config(&conn, &record1).unwrap();

        // Update the config
        let record2 = TtsConfigRecord {
            id: 0,
            engine: "kokoro".to_string(),
            default_voice: Some("af_bella".to_string()),
            speed: 1.5,
            format: "wav".to_string(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let id2 = upsert_tts_config(&conn, &record2).unwrap();

        // Same id returned (not a new record)
        assert_eq!(id1, id2);

        // Verify the update took effect
        let found = get_tts_config(&conn, "kokoro").unwrap().unwrap();
        assert_eq!(found.default_voice, Some("af_bella".to_string()));
        assert_eq!(found.speed, 1.5);
    }

    /// Test that delete_tts_config removes a record.
    #[test]
    fn test_delete_tts_config() {
        let conn = setup_test_db();

        upsert_tts_config(
            &conn,
            &TtsConfigRecord {
                id: 0,
                engine: "kokoro".into(),
                default_voice: None,
                speed: 1.0,
                format: "mp3".into(),
                enabled: true,
                created_at: String::new(),
                updated_at: String::new(),
            },
        )
        .unwrap();

        delete_tts_config(&conn, "kokoro").unwrap();

        let result = get_tts_config(&conn, "kokoro").unwrap();
        assert!(result.is_none());
    }

    /// Test that deleting a non-existent engine does not error.
    #[test]
    fn test_delete_nonexistent_engine() {
        let conn = setup_test_db();
        // Should not panic or error
        let result = delete_tts_config(&conn, "nonexistent");
        assert!(result.is_ok());
    }

    /// Test that enabled field is correctly stored as boolean.
    #[test]
    fn test_enabled_boolean_storage() {
        let conn = setup_test_db();

        // Insert with enabled=false (0)
        upsert_tts_config(
            &conn,
            &TtsConfigRecord {
                id: 0,
                engine: "kokoro".into(),
                default_voice: None,
                speed: 1.0,
                format: "mp3".into(),
                enabled: false,
                created_at: String::new(),
                updated_at: String::new(),
            },
        )
        .unwrap();

        let found = get_tts_config(&conn, "kokoro").unwrap().unwrap();
        assert!(!found.enabled);
    }

    /// Test that timestamps are stored as passed (upsert_tts_config passes
    /// the record's timestamps directly, so empty strings remain empty).
    #[test]
    fn test_timestamps_stored_as_passed() {
        let conn = setup_test_db();

        upsert_tts_config(
            &conn,
            &TtsConfigRecord {
                id: 0,
                engine: "kokoro".into(),
                default_voice: None,
                speed: 1.0,
                format: "mp3".into(),
                enabled: true,
                created_at: String::new(),
                updated_at: String::new(),
            },
        )
        .unwrap();

        let found = get_tts_config(&conn, "kokoro").unwrap().unwrap();
        // The upsert function passes the record's timestamps directly,
        // so empty strings are stored as-is.
        assert!(
            found.created_at.is_empty(),
            "created_at should be stored as passed (empty)"
        );
        assert!(
            found.updated_at.is_empty(),
            "updated_at should be stored as passed (empty)"
        );
    }
}
