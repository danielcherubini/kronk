//! Model configuration database query functions.

use anyhow::Result;
use rusqlite::{params, Connection};

use super::types::ModelConfigRecord;

/// Insert or update the model configuration.
/// Timestamp updated via SQLite's strftime('%Y-%m-%dT%H:%M:%fZ', 'now') on conflict.
/// Returns the model id.
pub fn upsert_model_config(conn: &Connection, record: &ModelConfigRecord) -> Result<i64> {
    conn.execute(
        "INSERT INTO model_configs (
            repo_id, display_name, backend, enabled, selected_quant,
            selected_mmproj, context_length, gpu_layers, port, args,
            sampling, modalities, profile, api_name, health_check,
            created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
        )
         ON CONFLICT(repo_id) DO UPDATE SET
             display_name = excluded.display_name,
             backend = excluded.backend,
             enabled = excluded.enabled,
             selected_quant = excluded.selected_quant,
             selected_mmproj = excluded.selected_mmproj,
             context_length = excluded.context_length,
             gpu_layers = excluded.gpu_layers,
             port = excluded.port,
             args = excluded.args,
             sampling = excluded.sampling,
             modalities = excluded.modalities,
             profile = excluded.profile,
             api_name = excluded.api_name,
             health_check = excluded.health_check,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            record.repo_id,
            record.display_name,
            record.backend,
            record.enabled as i32,
            record.selected_quant,
            record.selected_mmproj,
            record.context_length,
            record.gpu_layers,
            record.port,
            record.args,
            record.sampling,
            record.modalities,
            record.profile,
            record.api_name,
            record.health_check,
            record.created_at,
            record.updated_at,
        ],
    )?;
    // Return the id (either existing or newly created)
    let id: i64 = conn.query_row(
        "SELECT id FROM model_configs WHERE repo_id = ?1",
        [&record.repo_id],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Get the model configuration by id. Returns None if not found.
pub fn get_model_config(conn: &Connection, id: i64) -> Result<Option<ModelConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, repo_id, display_name, backend, enabled, selected_quant,
                selected_mmproj, context_length, gpu_layers, port, args,
                sampling, modalities, profile, api_name, health_check,
                created_at, updated_at
         FROM model_configs WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], |row| {
        Ok(ModelConfigRecord {
            id: row.get(0)?,
            repo_id: row.get(1)?,
            display_name: row.get(2)?,
            backend: row.get(3)?,
            enabled: row.get::<_, i32>(4)? != 0,
            selected_quant: row.get(5)?,
            selected_mmproj: row.get(6)?,
            context_length: row.get(7)?,
            num_parallel: None, // not yet in migration; will be read after migration v14
            gpu_layers: row.get(8)?,
            port: row.get(9)?,
            args: row.get(10)?,
            sampling: row.get(11)?,
            modalities: row.get(12)?,
            profile: row.get(13)?,
            api_name: row.get(14)?,
            health_check: row.get(15)?,
            created_at: row.get(16)?,
            updated_at: row.get(17)?,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Get the model configuration by repo_id. Returns None if not found.
pub fn get_model_config_by_repo_id(
    conn: &Connection,
    repo_id: &str,
) -> Result<Option<ModelConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, repo_id, display_name, backend, enabled, selected_quant,
                selected_mmproj, context_length, gpu_layers, port, args,
                sampling, modalities, profile, api_name, health_check,
                created_at, updated_at
         FROM model_configs WHERE repo_id = ?1",
    )?;
    let mut rows = stmt.query_map([repo_id], |row| {
        Ok(ModelConfigRecord {
            id: row.get(0)?,
            repo_id: row.get(1)?,
            display_name: row.get(2)?,
            backend: row.get(3)?,
            enabled: row.get::<_, i32>(4)? != 0,
            selected_quant: row.get(5)?,
            selected_mmproj: row.get(6)?,
            context_length: row.get(7)?,
            num_parallel: None, // not yet in migration; will be read after migration v14
            gpu_layers: row.get(8)?,
            port: row.get(9)?,
            args: row.get(10)?,
            sampling: row.get(11)?,
            modalities: row.get(12)?,
            profile: row.get(13)?,
            api_name: row.get(14)?,
            health_check: row.get(15)?,
            created_at: row.get(16)?,
            updated_at: row.get(17)?,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Get all stored model configurations.
pub fn get_all_model_configs(conn: &Connection) -> Result<Vec<ModelConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, repo_id, display_name, backend, enabled, selected_quant,
                selected_mmproj, context_length, gpu_layers, port, args,
                sampling, modalities, profile, api_name, health_check,
                created_at, updated_at
         FROM model_configs",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ModelConfigRecord {
            id: row.get(0)?,
            repo_id: row.get(1)?,
            display_name: row.get(2)?,
            backend: row.get(3)?,
            enabled: row.get::<_, i32>(4)? != 0,
            selected_quant: row.get(5)?,
            selected_mmproj: row.get(6)?,
            context_length: row.get(7)?,
            num_parallel: None, // not yet in migration; will be read after migration v14
            gpu_layers: row.get(8)?,
            port: row.get(9)?,
            args: row.get(10)?,
            sampling: row.get(11)?,
            modalities: row.get(12)?,
            profile: row.get(13)?,
            api_name: row.get(14)?,
            health_check: row.get(15)?,
            created_at: row.get(16)?,
            updated_at: row.get(17)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Delete the model configuration by id. CASCADE deletes model_pulls and model_files.
pub fn delete_model_config(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM model_configs WHERE id = ?1", [id])?;
    Ok(())
}
