//! Database module for SQLite
//!
//! Provides connection helpers and automatic migration system.

pub mod migrations;
pub mod queries;

use std::path::Path;

use rusqlite::Connection;

/// Open (or create) the SQLite database at `config_dir/kronk.db`
///
/// Sets up the database with:
/// - WAL mode enabled
/// - Foreign keys enabled
/// - Migrations applied
///
/// Returns a connection to the database.
pub fn open(config_dir: &Path) -> anyhow::Result<Connection> {
    let db_path = config_dir.join("kronk.db");
    let conn = Connection::open(&db_path)?;

    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    migrations::run(&conn)?;

    Ok(conn)
}

/// Open an in-memory SQLite database for testing
///
/// Applies the same PRAGMA settings as `open()` and runs migrations.
pub fn open_in_memory() -> anyhow::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    migrations::run(&conn)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let conn = open_in_memory().unwrap();

        // Verify tables exist by querying sqlite_master
        let pulls_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='model_pulls'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pulls_count, 1);

        let files_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='model_files'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(files_count, 1);

        let log_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='download_log'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(log_count, 1);

        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='idx_download_log_repo'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);
    }

    #[test]
    fn test_migrations_idempotent() {
        let conn = open_in_memory().unwrap();

        // Run migrations twice - should not error
        migrations::run(&conn).unwrap();
        migrations::run(&conn).unwrap();
    }

    #[test]
    fn test_user_version_updated() {
        let conn = open_in_memory().unwrap();

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, migrations::LATEST_VERSION);
    }
}
