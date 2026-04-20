use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::api::load_config_from_state;
use crate::server::AppState;

/// Resolve a model identifier string to an integer id.
/// Accepts either an integer id or a config_key (double-dash format).
pub(crate) fn resolve_model_id(
    id_str: &str,
    conn: &rusqlite::Connection,
) -> anyhow::Result<Option<i64>> {
    // Try parsing as integer id first
    if let Ok(id) = id_str.parse::<i64>() {
        return Ok(Some(id));
    }
    // Otherwise treat as config_key (double-dash format) → convert to repo_id and look up
    let repo_id = koji_core::db::config_key_to_repo_id(id_str);
    let record = koji_core::db::queries::get_model_config_by_repo_id(conn, &repo_id)?;
    Ok(record.map(|r| r.id))
}

/// Per-file DB metadata enrichment loaded from the `model_files` / `model_pulls`
/// SQLite tables. Layered onto the API response so the frontend can render
/// verification state, LFS hashes, and repo-level commit SHA without changing
/// the TOML schema.
#[derive(Debug, Default, Clone)]
struct RepoDbMeta {
    commit_sha: Option<String>,
    pulled_at: Option<String>,
    /// Keyed by filename (matches `QuantEntry.file`), not by quant name.
    files: std::collections::HashMap<String, koji_core::db::queries::ModelFileRecord>,
}

/// Load per-repo DB metadata for a model by its integer id.
fn load_repo_db_meta(config_dir: &std::path::Path, model_id: i64) -> RepoDbMeta {
    let Ok(open) = koji_core::db::open(config_dir) else {
        return RepoDbMeta::default();
    };
    let mut meta = RepoDbMeta::default();
    if let Ok(Some(pull)) = koji_core::db::queries::get_model_pull(&open.conn, model_id) {
        meta.commit_sha = Some(pull.commit_sha);
        meta.pulled_at = Some(pull.pulled_at);
    }
    if let Ok(files) = koji_core::db::queries::get_model_files(&open.conn, model_id) {
        for f in files {
            meta.files.insert(f.filename.clone(), f);
        }
    }
    meta
}

/// Build the full JSON for a model config entry, including all unified fields.
///
/// When `db_meta` is provided, each quant entry is enriched with its stored
/// LFS hash, DB-tracked size, and verification status, and the repo-level
/// commit SHA / last-pulled timestamp is surfaced at the top of the entry.
fn model_entry_json(
    id: i64,
    record: &koji_core::db::queries::ModelConfigRecord,
    m: &koji_core::config::ModelConfig,
    _configs_dir: &std::path::Path,
    backends: Option<&[String]>,
    db_meta: Option<&RepoDbMeta>,
) -> serde_json::Value {
    // Build a per-quant JSON map, layering DB metadata onto each entry by filename.
    let quants_json: serde_json::Map<String, serde_json::Value> = m
        .quants
        .iter()
        .map(|(name, q)| {
            let mut entry = serde_json::json!({
                "file": q.file,
                "kind": q.kind,
                "size_bytes": q.size_bytes,
                "context_length": q.context_length,
            });
            if let Some(meta) = db_meta.and_then(|dm| dm.files.get(&q.file)) {
                entry["lfs_oid"] = meta.lfs_oid.clone().into();
                entry["db_size_bytes"] = meta.size_bytes.into();
                entry["last_verified_at"] = meta.last_verified_at.clone().into();
                entry["verified_ok"] = meta.verified_ok.into();
                entry["verify_error"] = meta.verify_error.clone().into();
            }
            (name.clone(), entry)
        })
        .collect();

    let mut val = serde_json::json!({
        "id": id,
        "repo_id": record.repo_id,
        "backend": record.backend,
        "model": m.model,
        "quant": m.quant,
        "mmproj": m.mmproj,
        "args": m.args,
        "sampling": m.sampling,
        "enabled": record.enabled,
        "context_length": record.context_length,
        "num_parallel": record.num_parallel,
        "port": record.port,
        "api_name": record.api_name,
        "display_name": record.display_name,
        "gpu_layers": record.gpu_layers,
        "quants": quants_json,
        "modalities": m.modalities,
    });

    if let Some(meta) = db_meta {
        val["repo_commit_sha"] = meta.commit_sha.clone().into();
        val["repo_pulled_at"] = meta.pulled_at.clone().into();
    }

    if let Some(backends) = backends {
        val["backends"] = backends.to_vec().into();
    }

    val
}

/// GET /api/models — list all model configs plus available backends.
pub async fn list_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let state = state.clone();
    match tokio::task::spawn_blocking(move || load_config_from_state(&state)).await {
        Ok(Ok((cfg, config_dir))) => {
            let configs_dir = config_dir.join("configs");
            let backends: Vec<String> = cfg.backends.keys().cloned().collect();

            // Load models from DB — get records with integer ids
            let models = match koji_core::db::open(&config_dir) {
                Ok(open) => {
                    let records = koji_core::db::queries::get_all_model_configs(&open.conn)
                        .unwrap_or_default();
                    records
                        .iter()
                        .map(|record| {
                            let m = koji_core::config::ModelConfig::from_db_record(record);
                            let meta = load_repo_db_meta(&config_dir, record.id);
                            // Populate quants from model_files
                            let mut config = m.clone();
                            for f in meta.files.values() {
                                let quant_key =
                                    f.quant.clone().unwrap_or_else(|| f.filename.clone());
                                config.quants.insert(
                                    quant_key,
                                    koji_core::config::QuantEntry {
                                        file: f.filename.clone(),
                                        kind: koji_core::config::QuantKind::from_filename(
                                            &f.filename,
                                        ),
                                        size_bytes: f.size_bytes.map(|s| s as u64),
                                        context_length: None,
                                    },
                                );
                            }
                            model_entry_json(
                                record.id,
                                record,
                                &config,
                                &configs_dir,
                                None,
                                Some(&meta),
                            )
                        })
                        .collect::<Vec<_>>()
                }
                Err(_) => Vec::new(),
            };

            let sampling_templates: serde_json::Value =
                serde_json::to_value(&cfg.sampling_templates).unwrap_or_default();
            Json(serde_json::json!({
                "models": models,
                "backends": backends,
                "sampling_templates": sampling_templates
            }))
            .into_response()
        }
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// GET /api/models/:id — get a single model config.
/// Accepts integer id or config_key (double-dash format) for compatibility.
pub async fn get_model(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    match tokio::task::spawn_blocking(move || load_config_from_state(&state)).await {
        Ok(Ok((cfg, config_dir))) => {
            let configs_dir = config_dir.join("configs");
            let backends: Vec<String> = cfg.backends.keys().cloned().collect();

            // Resolve id (integer or config_key) to model_id
            let open = match koji_core::db::open(&config_dir) {
                Ok(o) => o,
                Err(_) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({"error": "Failed to open database"})),
                    )
                        .into_response();
                }
            };
            let model_id = match resolve_model_id(&id_str, &open.conn) {
                Ok(Some(id)) => id,
                Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(serde_json::json!({"error": "Model not found"})),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": e.to_string()})),
                    )
                        .into_response();
                }
            };

            // Load model from DB
            let model_opt = koji_core::db::queries::get_model_config(&open.conn, model_id)
                .ok()
                .flatten();

            match model_opt {
                Some(record) => {
                    let m = koji_core::config::ModelConfig::from_db_record(&record);
                    let mut config = m.clone();
                    let meta = load_repo_db_meta(&config_dir, record.id);
                    // Populate quants from model_files
                    for f in meta.files.values() {
                        let quant_key = f.quant.clone().unwrap_or_else(|| f.filename.clone());
                        config.quants.insert(
                            quant_key,
                            koji_core::config::QuantEntry {
                                file: f.filename.clone(),
                                kind: koji_core::config::QuantKind::from_filename(&f.filename),
                                size_bytes: f.size_bytes.map(|s| s as u64),
                                context_length: None,
                            },
                        );
                    }
                    let mut val = model_entry_json(
                        record.id,
                        &record,
                        &config,
                        &configs_dir,
                        Some(&backends),
                        Some(&meta),
                    );
                    val["backends"] = backends.into();
                    Json(val).into_response()
                }
                None => (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error": "Model not found"})),
                )
                    .into_response(),
            }
        }
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
