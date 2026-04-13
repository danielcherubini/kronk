//! Database module for SQLite
//!
//! Provides connection helpers and automatic migration system.

pub mod backfill;
pub mod migrations;
pub mod queries;

use std::path::Path;

use anyhow::Context;
use rusqlite::Connection;

/// Result of opening a database connection
pub struct OpenResult {
    pub conn: Connection,
    pub needs_backfill: bool,
}

/// Open (or create) the SQLite database at `config_dir/koji.db`
///
/// Sets up the database with:
/// - WAL mode enabled
/// - Foreign keys enabled
/// - Migrations applied
///
/// Returns a connection and whether backfill is needed (true if DB was freshly created).
pub fn open(config_dir: &Path) -> anyhow::Result<OpenResult> {
    // Ensure the config directory exists before SQLite tries to create the file.
    std::fs::create_dir_all(config_dir)?;
    let db_path = config_dir.join("koji.db");
    let conn = Connection::open(&db_path)?;

    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    // Check user_version BEFORE running migrations to detect fresh DB
    let current_version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let needs_backfill = current_version == 0;

    migrations::run(&conn)?;

    Ok(OpenResult {
        conn,
        needs_backfill,
    })
}

/// Backup the SQLite database at `config_dir/koji.db` to a destination path.
///
/// Uses SQLite's `VACUUM INTO` command to create a clean, consistent copy of
/// the database. This avoids copying WAL/SHM files and guarantees a consistent
/// snapshot even if the database is in use.
///
/// # Arguments
/// * `config_dir` - The koji config directory containing `koji.db`
/// * `dest` - Where to write the backup database file
///
/// # Returns
/// Result<()> indicating success or failure
pub fn backup_db(config_dir: &Path, dest: &Path) -> anyhow::Result<()> {
    // Compute safe parent path - avoid creating directory named after the file
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent)
        .context("Failed to create parent directory for backup")?;

    let db_path = config_dir.join("koji.db");
    let conn = Connection::open(&db_path)?;

    // VACUUM INTO creates a clean copy without WAL/SHM files
    // Convert Path to string for rusqlite parameter binding
    let dest_str = dest.to_string_lossy().to_string();
    conn.execute("VACUUM INTO ?", [&dest_str])
        .context("Failed to vacuum database into destination")?;

    Ok(())
}

/// Open an in-memory SQLite database for testing.
///
/// Applies `PRAGMA foreign_keys=ON` (same as `open()`) and runs migrations.
/// Note: `journal_mode=WAL` is not applied because it is not supported for
/// in-memory databases.
pub fn open_in_memory() -> anyhow::Result<OpenResult> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    // In-memory DB starts at version 0, so it needs backfill
    let current_version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let needs_backfill = current_version == 0;

    migrations::run(&conn)?;

    Ok(OpenResult {
        conn,
        needs_backfill,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

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
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Run migrations twice - should not error
        migrations::run(&conn).unwrap();
        migrations::run(&conn).unwrap();
    }

    #[test]
    fn test_user_version_updated() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, migrations::LATEST_VERSION);
    }

    #[test]
    fn test_migration_v3_creates_backend_installations() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='backend_installations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "backend_installations table should exist after migration v3"
        );

        // Verify index was created
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_backend_installations_name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            idx_count, 1,
            "idx_backend_installations_name index should exist after migration v3"
        );
    }

    /// Test that migration v4 creates the `system_metrics_history` table and its index.
    #[test]
    fn test_migration_v4_creates_system_metrics_history() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='system_metrics_history'",
            [], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
        let idx_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_system_metrics_ts'",
            [], |row| row.get(0)).unwrap();
        assert_eq!(idx_count, 1);
    }
}
