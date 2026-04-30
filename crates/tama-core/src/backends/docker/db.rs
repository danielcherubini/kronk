/// Database helpers for Docker backends.
///
/// Provides functions to look up Docker backends by name from the SQLite DB.
use anyhow::{anyhow, Context, Result};
use std::path::Path;

use super::DockerBackend;

/// Look up a Docker backend by name from the DB.
///
/// Requires the DB directory path to open the connection.
/// Returns `Ok(None)` if no backend with that name exists.
pub async fn get_backend_by_name(name: &str, db_dir: &Path) -> Result<Option<DockerBackend>> {
    let conn = crate::db::open(db_dir)
        .with_context(|| format!("Failed to open DB at {}", db_dir.display()))?;

    // Query the backend_installations table for a Docker backend
    let mut stmt = conn.conn.prepare(
        "SELECT compose_yaml, dockerfile, target_port
         FROM backend_installations
         WHERE name = ?1 AND backend_type = 'docker' AND is_active = 1",
    )?;

    let row = stmt.query_row([name], |row| {
        Ok(DockerBackend {
            name: name.to_string(),
            compose_yaml: row.get(0)?,
            dockerfile: row.get(1)?,
            target_port: row.get(2)?,
            config_dir: db_dir.to_path_buf(),
        })
    });

    match row {
        Ok(backend) => Ok(Some(backend)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(anyhow!("Failed to query Docker backend: {}", e)),
    }
}
