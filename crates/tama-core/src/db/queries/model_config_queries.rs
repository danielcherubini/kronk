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
            repo_id, display_name, backend, gpu_variant, enabled, selected_quant,
            selected_mmproj, context_length, num_parallel, kv_unified, gpu_layers,
            cache_type_k, cache_type_v, port, args,
            sampling, modalities, profile, api_name, health_check,
            hf_format, hf_base_model, hf_pipeline_tag, hf_total_params,
            hf_active_params, hf_architecture_type, hf_context_length,
            hf_num_layers, hf_last_modified,
            created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
            ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29,
            ?30, ?31
        )
         ON CONFLICT(repo_id) DO UPDATE SET
             display_name = excluded.display_name,
             backend = excluded.backend,
             gpu_variant = excluded.gpu_variant,
             enabled = excluded.enabled,
             selected_quant = excluded.selected_quant,
             selected_mmproj = excluded.selected_mmproj,
             context_length = excluded.context_length,
             num_parallel = excluded.num_parallel,
             kv_unified = excluded.kv_unified,
             gpu_layers = excluded.gpu_layers,
             cache_type_k = excluded.cache_type_k,
             cache_type_v = excluded.cache_type_v,
             port = excluded.port,
             args = excluded.args,
             sampling = excluded.sampling,
             modalities = excluded.modalities,
             profile = excluded.profile,
             api_name = excluded.api_name,
             health_check = excluded.health_check,
             /* HF metadata: use COALESCE to preserve existing values when the
                upsert record has NULL (e.g. during scan/pull which doesn't fetch HF data) */
             hf_format = COALESCE(excluded.hf_format, hf_format),
             hf_base_model = COALESCE(excluded.hf_base_model, hf_base_model),
             hf_pipeline_tag = COALESCE(excluded.hf_pipeline_tag, hf_pipeline_tag),
             hf_total_params = COALESCE(excluded.hf_total_params, hf_total_params),
             hf_active_params = COALESCE(excluded.hf_active_params, hf_active_params),
             hf_architecture_type = COALESCE(excluded.hf_architecture_type, hf_architecture_type),
             hf_context_length = COALESCE(excluded.hf_context_length, hf_context_length),
             hf_num_layers = COALESCE(excluded.hf_num_layers, hf_num_layers),
             hf_last_modified = COALESCE(excluded.hf_last_modified, hf_last_modified),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![
            record.repo_id,
            record.display_name,
            record.backend,
            record.gpu_variant,
            record.enabled as i32,
            record.selected_quant,
            record.selected_mmproj,
            record.context_length,
            record.num_parallel,
            record.kv_unified as i32,
            record.gpu_layers,
            record.cache_type_k,
            record.cache_type_v,
            record.port,
            record.args,
            record.sampling,
            record.modalities,
            record.profile,
            record.api_name,
            record.health_check,
            record.hf_format,
            record.hf_base_model,
            record.hf_pipeline_tag,
            record.hf_total_params,
            record.hf_active_params,
            record.hf_architecture_type,
            record.hf_context_length,
            record.hf_num_layers,
            record.hf_last_modified,
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
        "SELECT id, repo_id, display_name, backend, gpu_variant, enabled, selected_quant,
                selected_mmproj, context_length, num_parallel, kv_unified, gpu_layers,
                cache_type_k, cache_type_v, port, args,
                sampling, modalities, profile, api_name, health_check,
                hf_format, hf_base_model, hf_pipeline_tag, hf_total_params,
                hf_active_params, hf_architecture_type, hf_context_length,
                hf_num_layers, hf_last_modified,
                created_at, updated_at
         FROM model_configs WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], |row| {
        Ok(ModelConfigRecord {
            id: row.get(0)?,
            repo_id: row.get(1)?,
            display_name: row.get(2)?,
            backend: row.get(3)?,
            gpu_variant: row.get(4)?,
            enabled: row.get::<_, i32>(5)? != 0,
            selected_quant: row.get(6)?,
            selected_mmproj: row.get(7)?,
            context_length: row.get(8)?,
            num_parallel: row.get(9)?,
            kv_unified: row.get::<_, i32>(10)? != 0,
            gpu_layers: row.get(11)?,
            cache_type_k: row.get(12)?,
            cache_type_v: row.get(13)?,
            port: row.get(14)?,
            args: row.get(15)?,
            sampling: row.get(16)?,
            modalities: row.get(17)?,
            profile: row.get(18)?,
            api_name: row.get(19)?,
            health_check: row.get(20)?,
            hf_format: row.get(21)?,
            hf_base_model: row.get(22)?,
            hf_pipeline_tag: row.get(23)?,
            hf_total_params: row.get(24)?,
            hf_active_params: row.get(25)?,
            hf_architecture_type: row.get(26)?,
            hf_context_length: row.get(27)?,
            hf_num_layers: row.get(28)?,
            hf_last_modified: row.get(29)?,
            created_at: row.get(30)?,
            updated_at: row.get(31)?,
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
        "SELECT id, repo_id, display_name, backend, gpu_variant, enabled, selected_quant,
                selected_mmproj, context_length, num_parallel, kv_unified, gpu_layers,
                cache_type_k, cache_type_v, port, args,
                sampling, modalities, profile, api_name, health_check,
                hf_format, hf_base_model, hf_pipeline_tag, hf_total_params,
                hf_active_params, hf_architecture_type, hf_context_length,
                hf_num_layers, hf_last_modified,
                created_at, updated_at
         FROM model_configs WHERE repo_id = ?1",
    )?;
    let mut rows = stmt.query_map([repo_id], |row| {
        Ok(ModelConfigRecord {
            id: row.get(0)?,
            repo_id: row.get(1)?,
            display_name: row.get(2)?,
            backend: row.get(3)?,
            gpu_variant: row.get(4)?,
            enabled: row.get::<_, i32>(5)? != 0,
            selected_quant: row.get(6)?,
            selected_mmproj: row.get(7)?,
            context_length: row.get(8)?,
            num_parallel: row.get(9)?,
            kv_unified: row.get::<_, i32>(10)? != 0,
            gpu_layers: row.get(11)?,
            cache_type_k: row.get(12)?,
            cache_type_v: row.get(13)?,
            port: row.get(14)?,
            args: row.get(15)?,
            sampling: row.get(16)?,
            modalities: row.get(17)?,
            profile: row.get(18)?,
            api_name: row.get(19)?,
            health_check: row.get(20)?,
            hf_format: row.get(21)?,
            hf_base_model: row.get(22)?,
            hf_pipeline_tag: row.get(23)?,
            hf_total_params: row.get(24)?,
            hf_active_params: row.get(25)?,
            hf_architecture_type: row.get(26)?,
            hf_context_length: row.get(27)?,
            hf_num_layers: row.get(28)?,
            hf_last_modified: row.get(29)?,
            created_at: row.get(30)?,
            updated_at: row.get(31)?,
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
        "SELECT id, repo_id, display_name, backend, gpu_variant, enabled, selected_quant,
                selected_mmproj, context_length, num_parallel, kv_unified, gpu_layers,
                cache_type_k, cache_type_v, port, args,
                sampling, modalities, profile, api_name, health_check,
                hf_format, hf_base_model, hf_pipeline_tag, hf_total_params,
                hf_active_params, hf_architecture_type, hf_context_length,
                hf_num_layers, hf_last_modified,
                created_at, updated_at
         FROM model_configs",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ModelConfigRecord {
            id: row.get(0)?,
            repo_id: row.get(1)?,
            display_name: row.get(2)?,
            backend: row.get(3)?,
            gpu_variant: row.get(4)?,
            enabled: row.get::<_, i32>(5)? != 0,
            selected_quant: row.get(6)?,
            selected_mmproj: row.get(7)?,
            context_length: row.get(8)?,
            num_parallel: row.get(9)?,
            kv_unified: row.get::<_, i32>(10)? != 0,
            gpu_layers: row.get(11)?,
            cache_type_k: row.get(12)?,
            cache_type_v: row.get(13)?,
            port: row.get(14)?,
            args: row.get(15)?,
            sampling: row.get(16)?,
            modalities: row.get(17)?,
            profile: row.get(18)?,
            api_name: row.get(19)?,
            health_check: row.get(20)?,
            hf_format: row.get(21)?,
            hf_base_model: row.get(22)?,
            hf_pipeline_tag: row.get(23)?,
            hf_total_params: row.get(24)?,
            hf_active_params: row.get(25)?,
            hf_architecture_type: row.get(26)?,
            hf_context_length: row.get(27)?,
            hf_num_layers: row.get(28)?,
            hf_last_modified: row.get(29)?,
            created_at: row.get(30)?,
            updated_at: row.get(31)?,
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
