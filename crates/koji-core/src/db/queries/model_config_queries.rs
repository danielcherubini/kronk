//! Model configuration database query functions.

use anyhow::Result;
use rusqlite::{params, Connection};

use super::types::ModelConfigRecord;

/// Insert or update the model configuration for a repo.
/// Timestamp updated via SQLite's strftime('%Y-%m-%dT%H:%M:%fZ', 'now') on conflict.
pub fn upsert_model_config(conn: &Connection, record: &ModelConfigRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO model_configs (
            repo_id, display_name, backend, enabled, selected_quant,
            selected_mmproj, context_length, gpu_layers, port, args,
            sampling, modalities, profile, api_name, health_check,
            created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
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
    Ok(())
}

/// Get the model configuration for a repo. Returns None if not found.
pub fn get_model_config(conn: &Connection, repo_id: &str) -> Result<Option<ModelConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT repo_id, display_name, backend, enabled, selected_quant,
                selected_mmproj, context_length, gpu_layers, port, args,
                sampling, modalities, profile, api_name, health_check,
                created_at, updated_at
         FROM model_configs WHERE repo_id = ?1",
    )?;
    let mut rows = stmt.query_map([repo_id], |row| {
        Ok(ModelConfigRecord {
            repo_id: row.get(0)?,
            display_name: row.get(1)?,
            backend: row.get(2)?,
            enabled: row.get::<_, i32>(3)? != 0,
            selected_quant: row.get(4)?,
            selected_mmproj: row.get(5)?,
            context_length: row.get(6)?,
            gpu_layers: row.get(7)?,
            port: row.get(8)?,
            args: row.get(9)?,
            sampling: row.get(10)?,
            modalities: row.get(11)?,
            profile: row.get(12)?,
            api_name: row.get(13)?,
            health_check: row.get(14)?,
            created_at: row.get(15)?,
            updated_at: row.get(16)?,
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
        "SELECT repo_id, display_name, backend, enabled, selected_quant,
                selected_mmproj, context_length, gpu_layers, port, args,
                sampling, modalities, profile, api_name, health_check,
                created_at, updated_at
         FROM model_configs",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ModelConfigRecord {
            repo_id: row.get(0)?,
            display_name: row.get(1)?,
            backend: row.get(2)?,
            enabled: row.get::<_, i32>(3)? != 0,
            selected_quant: row.get(4)?,
            selected_mmproj: row.get(5)?,
            context_length: row.get(6)?,
            gpu_layers: row.get(7)?,
            port: row.get(8)?,
            args: row.get(9)?,
            sampling: row.get(10)?,
            modalities: row.get(11)?,
            profile: row.get(12)?,
            api_name: row.get(13)?,
            health_check: row.get(14)?,
            created_at: row.get(15)?,
            updated_at: row.get(16)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Delete the model configuration for a repo and all associated model files.
/// Runs in a single transaction.
pub fn delete_model_config(conn: &Connection, repo_id: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM model_configs WHERE repo_id = ?1", [repo_id])?;
    super::model_queries::_delete_model_records(&tx, repo_id)?;
    tx.commit()?;
    Ok(())
}
