use axum::extract::Path;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use crate::server::AppState;

fn resolve_model_id(id_str: &str, conn: &rusqlite::Connection) -> anyhow::Result<Option<i64>> {
    // Try parsing as integer id first
    if let Ok(id) = id_str.parse::<i64>() {
        return Ok(Some(id));
    }
    // Otherwise treat as config_key (double-dash format) → convert to repo_id and look up
    let repo_id = koji_core::db::config_key_to_repo_id(id_str);
    let record = koji_core::db::queries::get_model_config_by_repo_id(conn, &repo_id)?;
    Ok(record.map(|r| r.id))
}

pub mod backends;
pub mod backup;
pub mod middleware;
pub mod self_update;
pub mod updates;

/// Query parameters for GET /api/logs
#[derive(serde::Deserialize)]
pub struct LogsQuery {
    /// Number of lines to return (default: 200)
    #[serde(default = "default_lines")]
    pub lines: usize,
}
fn default_lines() -> usize {
    200
}

pub async fn get_logs(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> impl IntoResponse {
    let dir = match &state.logs_dir {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "logs_dir not configured"})),
            )
                .into_response()
        }
    };
    let log_path = dir.join("koji.log");
    // Use spawn_blocking for synchronous file I/O to avoid blocking the Tokio runtime.
    let log_path_clone = log_path.clone();
    let n = query.lines;
    let lines = tokio::task::spawn_blocking(move || {
        koji_core::logging::tail_lines(&log_path_clone, n).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    Json(serde_json::json!({ "lines": lines })).into_response()
}

pub async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response()
        }
    };
    // Use spawn_blocking for synchronous file I/O.
    match tokio::task::spawn_blocking(move || std::fs::read_to_string(&path)).await {
        Ok(Ok(content)) => Json(serde_json::json!({ "content": content })).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct ConfigBody {
    pub content: String,
}

/// Update the proxy's live in-memory config after a successful disk save.
/// No-op if proxy_config is None (standalone web server without proxy).
async fn sync_proxy_config(state: &AppState, new_config: koji_core::config::Config) {
    if let Some(ref proxy_config) = state.proxy_config {
        let mut config = proxy_config.write().await;
        *config = new_config;
    }
}

/// Trigger the proxy to reload its model registry from the database.
async fn trigger_proxy_reload(state: &AppState) -> Result<(), (StatusCode, serde_json::Value)> {
    let url = format!("{}/koji/v1/system/reload-configs", state.proxy_base_url);
    let resp = state.client.post(&url).send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            serde_json::json!({"error": format!("Failed to reach proxy: {}", e)}),
        )
    })?;

    if !resp.status().is_success() {
        return Err((
            resp.status(),
            serde_json::json!({"error": "Proxy failed to reload configurations"}),
        ));
    }
    Ok(())
}

/// Body for structured config save.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StructuredConfigBody {
    pub general: crate::types::config::General,
    #[serde(default)]
    pub backends: std::collections::BTreeMap<String, crate::types::config::BackendConfig>,
    #[serde(default)]
    pub models: std::collections::BTreeMap<String, crate::types::config::ModelConfig>,
    #[serde(default)]
    pub supervisor: crate::types::config::Supervisor,
    #[serde(default)]
    pub sampling_templates:
        std::collections::BTreeMap<String, crate::types::config::SamplingParams>,
    #[serde(default)]
    pub proxy: crate::types::config::ProxyConfig,
}

pub async fn save_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ConfigBody>,
) -> impl IntoResponse {
    let path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response()
        }
    };
    // Validate TOML by parsing. Note: koji_core::config::Config has required fields
    // (e.g. `general`), so a partial TOML that omits top-level tables will fail here.
    // This is intentional — only fully valid config files are accepted.
    if let Err(e) = toml::from_str::<koji_core::config::Config>(&body.content) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": format!("Invalid TOML: {e}")})),
        )
            .into_response();
    }
    // Keep a copy of the validated content for syncing after the write.
    let content_for_sync = body.content.clone();
    // Use spawn_blocking for synchronous file I/O.
    match tokio::task::spawn_blocking(move || std::fs::write(&path, &body.content)).await {
        Ok(Ok(_)) => {
            // Parse the validated TOML into a Config and sync the proxy's live config.
            if let Ok(mut new_config) =
                toml::from_str::<koji_core::config::Config>(&content_for_sync)
            {
                // Restore loaded_from from the existing proxy config (it is skipped by serde).
                if let Some(ref proxy_config) = state.proxy_config {
                    new_config.loaded_from = proxy_config.read().await.loaded_from.clone();
                }
                sync_proxy_config(&state, new_config).await;
            }
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Structured Config API (JSON-based for WASM) ─────────────────────────────────

/// GET /api/config/structured — returns full Config as JSON.
pub async fn get_structured_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response();
        }
    };

    let config_dir = match config_path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Cannot determine config directory"})),
            )
                .into_response();
        }
    };

    // Load config from disk using koji_core (SSR-only path)
    let cfg = match tokio::task::spawn_blocking(move || {
        koji_core::config::Config::load_from(&config_dir)
    })
    .await
    {
        Ok(Ok(cfg)) => cfg,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Convert to mirror types for JSON serialization
    let structured: crate::types::config::Config = cfg.into();

    Json(structured).into_response()
}

/// POST /api/config/structured — accept JSON Config, persist as TOML.
pub async fn save_structured_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<StructuredConfigBody>,
) -> impl IntoResponse {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response();
        }
    };

    let config_dir = match config_path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Cannot determine config directory"})),
            )
                .into_response();
        }
    };

    // Convert mirror types back to koji_core::Config
    let mut new_config: koji_core::config::Config = body.into();

    // Restore loaded_from from existing proxy config (it has #[serde(skip)])
    if let Some(ref proxy_config) = state.proxy_config {
        new_config.loaded_from = proxy_config.read().await.loaded_from.clone();
    }

    // Persist to disk using koji_core's save_to (consistent with other endpoints)
    let config_dir_clone = config_dir.clone();
    let new_config_clone = new_config.clone();
    match tokio::task::spawn_blocking(move || new_config_clone.save_to(&config_dir_clone)).await {
        Ok(Ok(_)) => {
            // Sync proxy config for hot-reload
            sync_proxy_config(&state, new_config).await;
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Model CRUD ────────────────────────────────────────────────────────────────

/// Load config from the config_path stored in AppState.
/// Returns (config, config_dir) on success.
fn load_config_from_state(
    state: &AppState,
) -> Result<(koji_core::config::Config, std::path::PathBuf), (StatusCode, serde_json::Value)> {
    let config_path = state.config_path.as_ref().ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            serde_json::json!({"error": "config_path not configured"}),
        )
    })?;
    let config_dir = config_path
        .parent()
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": "Cannot determine config directory"}),
            )
        })?
        .to_path_buf();
    let cfg = koji_core::config::Config::load_from(&config_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}),
        )
    })?;
    Ok((cfg, config_dir))
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

/// Body for create/update model.
#[derive(serde::Deserialize)]
pub struct ModelBody {
    pub backend: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub mmproj: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sampling: Option<koji_core::profiles::SamplingParams>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub api_name: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub quants: Option<std::collections::BTreeMap<String, koji_core::config::QuantEntry>>,
    #[serde(default)]
    pub modalities: Option<koji_core::config::ModelModalities>,
}

fn apply_model_body(
    body: ModelBody,
    existing: Option<koji_core::config::ModelConfig>,
) -> koji_core::config::ModelConfig {
    let base = existing.unwrap_or_else(|| koji_core::config::ModelConfig {
        backend: String::new(),
        args: vec![],
        sampling: None,
        model: None,
        quant: None,
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
    });

    // Handle sampling from body
    let sampling = body.sampling;

    koji_core::config::ModelConfig {
        backend: body.backend,
        model: body.model,
        quant: body.quant,
        mmproj: body.mmproj,
        args: body.args,
        sampling,
        enabled: body.enabled.unwrap_or(base.enabled),
        context_length: body.context_length,
        port: body.port,
        health_check: base.health_check,
        profile: None,
        api_name: body.api_name,
        gpu_layers: body.gpu_layers,
        modalities: body.modalities,
        display_name: body.display_name,
        // Preserve server-side `size_bytes` on update: the UI exposes the field
        // read-only and callers must not be able to rewrite it via the API. The
        // authoritative value comes from the download pipeline
        // (`std::fs::metadata` after pull + the HF blob metadata that later
        // populates `model_files.size_bytes` during verify/refresh). If no
        // prior entry exists, accept the client's value to avoid regressing
        // freshly-created entries that don't yet have a stored size.
        quants: body
            .quants
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| {
                let preserved_size = base
                    .quants
                    .get(&k)
                    .and_then(|existing| existing.size_bytes)
                    .or(v.size_bytes);
                (
                    k,
                    koji_core::config::QuantEntry {
                        file: v.file,
                        kind: v.kind,
                        size_bytes: preserved_size,
                        context_length: v.context_length,
                    },
                )
            })
            .collect(),
    }
}

/// PUT /api/models/:id — update an existing model.
pub async fn update_model(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
    Json(body): Json<ModelBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (_cfg, config_dir) = load_config_from_state(&state)?;

        // Load existing from DB
        let open = koji_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        let model_id = resolve_model_id(&id_str, &open.conn)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let existing_record = koji_core::db::queries::get_model_config(&open.conn, model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let existing = koji_core::config::ModelConfig::from_db_record(&existing_record);

        let updated_config = apply_model_body(body, Some(existing));

        // Save to DB (save_model_config converts config_key to repo_id internally)
        let config_key = existing_record.repo_id.to_lowercase().replace('/', "--");
        let new_model_id =
            koji_core::db::save_model_config(&open.conn, &config_key, &updated_config).map_err(
                |e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        serde_json::json!({"error": e.to_string()}),
                    )
                },
            )?;
        Ok(serde_json::json!({ "ok": true, "id": new_model_id }))
    })
    .await
    {
        Ok(Ok(val)) => {
            // Since we only updated the DB, the proxy config (which is just General, Backends, etc.)
            // doesn't need syncing. But the proxy's runtime model registry DOES.
            if let Err(e) = trigger_proxy_reload(&state_clone).await {
                tracing::warn!("Failed to trigger proxy reload after update: {}", e.1);
            }
            Json(val).into_response()
        }
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /api/models — create a new model.
/// The body contains `repo_id` (HuggingFace repo name). Returns the auto-generated integer id.
#[derive(serde::Deserialize)]
pub struct CreateModelBody {
    pub repo_id: String,
    #[serde(flatten)]
    pub model: ModelBody,
}

pub async fn create_model(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateModelBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (_, config_dir) = load_config_from_state(&state)?;
        let repo_id = body.repo_id.trim().to_string();
        if repo_id.is_empty() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "repo_id cannot be empty"}),
            ));
        }

        let open = koji_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        if koji_core::db::queries::get_model_config_by_repo_id(&open.conn, &repo_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .is_some()
        {
            return Err((
                StatusCode::CONFLICT,
                serde_json::json!({"error": format!("Model '{}' already exists", repo_id)}),
            ));
        }

        let model_config = apply_model_body(body.model, None);
        let model_id = koji_core::db::save_model_config(&open.conn, &repo_id, &model_config)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?;

        Ok(serde_json::json!({ "ok": true, "id": model_id }))
    })
    .await
    {
        Ok(Ok(val)) => {
            if let Err(e) = trigger_proxy_reload(&state_clone).await {
                tracing::warn!("Failed to trigger proxy reload after create: {}", e.1);
            }
            (StatusCode::CREATED, Json(val)).into_response()
        }
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Body for rename endpoint.
#[derive(serde::Deserialize)]
pub struct RenameBody {
    pub new_repo_id: String,
}

/// POST /api/models/:id/rename — rename a model config entry.
pub async fn rename_model(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
    Json(body): Json<RenameBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (_, config_dir) = load_config_from_state(&state)?;

        let open = koji_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;

        // Check source ID exists
        let model_id = resolve_model_id(&id_str, &open.conn)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let existing_record = koji_core::db::queries::get_model_config(&open.conn, model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let model_config = koji_core::config::ModelConfig::from_db_record(&existing_record);

        let new_repo_id = body.new_repo_id.trim().to_string();
        if new_repo_id.is_empty() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "New repo_id cannot be empty"}),
            ));
        }

        // Check target repo_id doesn't already exist
        if koji_core::db::queries::get_model_config_by_repo_id(&open.conn, &new_repo_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .is_some()
        {
            return Err((
                StatusCode::CONFLICT,
                serde_json::json!({"error": format!("Model '{}' already exists", new_repo_id)}),
            ));
        }

        // Save with new repo_id (keeps same integer id)
        let config_key = new_repo_id.to_lowercase().replace('/', "--");
        let _ = koji_core::db::save_model_config(&open.conn, &config_key, &model_config).map_err(
            |e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            },
        )?;

        // Clean up update_check record for old repo_id
        let _ = koji_core::db::queries::delete_update_check(
            &open.conn,
            "model",
            &existing_record.repo_id,
        );

        Ok(serde_json::json!({ "ok": true, "id": model_id }))
    })
    .await
    {
        Ok(Ok(val)) => {
            if let Err(e) = trigger_proxy_reload(&state_clone).await {
                tracing::warn!("Failed to trigger proxy reload after rename: {}", e.1);
            }
            Json(val).into_response()
        }
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// DELETE /api/models/:id/quants/:quant_key — delete a single quant's file
/// and remove it from the config.
pub async fn delete_quant(
    State(state): State<Arc<AppState>>,
    Path((id, quant_key)): Path<(i64, String)>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (cfg, config_dir) = load_config_from_state(&state)?;

        let open = koji_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;

        // Find the model from DB
        let model_record = koji_core::db::queries::get_model_config(&open.conn, id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;

        let mut model_config = koji_core::config::ModelConfig::from_db_record(&model_record);

        // Find the quant entry
        let quant_entry = model_config.quants.get(&quant_key).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                serde_json::json!({"error": "Quant not found"}),
            )
        })?;

        // Clone the filename and repo_id before we mutate
        let filename = quant_entry.file.clone();
        let repo_id = model_record.repo_id.clone();

        // Clear active quant/mmproj if they referenced this quant
        if model_config.quant.as_deref() == Some(&quant_key) {
            model_config.quant = None;
        }
        if model_config.mmproj.as_deref() == Some(&quant_key) {
            model_config.mmproj = None;
        }

        // Remove the quant entry
        model_config.quants.remove(&quant_key);

        // Save to DB
        let config_key = repo_id.to_lowercase().replace('/', "--");
        koji_core::db::save_model_config(&open.conn, &config_key, &model_config).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;

        // Clean up file (best-effort) - only after config is saved
        if !repo_id.is_empty() {
            if let Ok(models_dir) = cfg.models_dir() {
                let file_path = koji_core::models::repo_path(&models_dir, &repo_id).join(&filename);
                if file_path.exists() {
                    if let Err(e) = std::fs::remove_file(&file_path) {
                        tracing::warn!(
                            "Failed to delete quant file {}: {}",
                            file_path.display(),
                            e
                        );
                    }
                }
            }
        }

        // Clean up DB record (best-effort) - only after config is saved
        if !repo_id.is_empty() {
            // We already have 'open' connection
            let _ = koji_core::db::queries::delete_model_file(&open.conn, id, &filename);
        }

        Ok((
            cfg,
            serde_json::json!({
                "ok": true,
                "id": id,
                "quant_key": quant_key,
                "deleted_file": filename
            }),
        ))
    })
    .await
    {
        Ok(Ok((_cfg, val))) => {
            if let Err(e) = trigger_proxy_reload(&state_clone).await {
                tracing::warn!("Failed to trigger proxy reload after delete_quant: {}", e.1);
            }
            Json(val).into_response()
        }
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// DELETE /api/models/:id — delete a model.
pub async fn delete_model(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (cfg, config_dir) = load_config_from_state(&state)?;

        // Capture the removed model for cleanup
        let open = koji_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        let model_id = resolve_model_id(&id_str, &open.conn)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let model_record = koji_core::db::queries::get_model_config(&open.conn, model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let _model_config = koji_core::config::ModelConfig::from_db_record(&model_record);

        // File cleanup (mirrors CLI model rm logic)
        let repo_id = model_record.repo_id.clone();
        if !repo_id.is_empty() {
            // 1. Delete model directory: models_dir / repo_id
            if let Ok(models_dir) = cfg.models_dir() {
                let model_dir = koji_core::models::repo_path(&models_dir, &repo_id);
                if model_dir.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&model_dir) {
                        tracing::warn!(
                            "Failed to remove model directory {}: {}",
                            model_dir.display(),
                            e
                        );
                    } else {
                        // Clean up empty parent dir
                        if let Some(parent) = model_dir.parent() {
                            if parent
                                .read_dir()
                                .map(|mut d| d.next().is_none())
                                .unwrap_or(false)
                            {
                                let _ = std::fs::remove_dir(parent);
                            }
                        }
                    }
                }
            }
            // 2. Delete model card
            if let Ok(configs_dir) = cfg.configs_dir() {
                let card_path = configs_dir.join(format!("{}.toml", repo_id.replace('/', "--")));
                if card_path.exists() {
                    let _ = std::fs::remove_file(&card_path);
                }
            }
            // 3. Delete DB records (best-effort)
            koji_core::db::queries::delete_model_records(&open.conn, model_id).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?;
        }

        // Delete the model config record and update check record
        koji_core::db::queries::delete_model_config(&open.conn, model_id).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        let _ = koji_core::db::queries::delete_update_check(&open.conn, "model", &repo_id);

        Ok(serde_json::json!({ "ok": true }))
    })
    .await
    {
        Ok(Ok(val)) => {
            if let Err(e) = trigger_proxy_reload(&state_clone).await {
                tracing::warn!("Failed to trigger proxy reload after delete: {}", e.1);
            }
            Json(val).into_response()
        }
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Serialize a `ModelFileRecord` into the same shape used by the enriched
/// quants response so refresh/verify callers get data identical to a GET.
fn file_record_json(rec: &koji_core::db::queries::ModelFileRecord) -> serde_json::Value {
    serde_json::json!({
        "filename": rec.filename,
        "quant": rec.quant,
        "lfs_oid": rec.lfs_oid,
        "size_bytes": rec.size_bytes,
        "downloaded_at": rec.downloaded_at,
        "last_verified_at": rec.last_verified_at,
        "verified_ok": rec.verified_ok,
        "verify_error": rec.verify_error,
    })
}

// ── Refresh / Verify ──────────────────────────────────────────────────────────

/// POST /api/models/:id/refresh — re-query HuggingFace for the current commit
/// SHA and per-file LFS hashes / sizes, and write them into the local DB.
///
/// Structured to keep `rusqlite::Connection` off `.await` points:
///   1. `spawn_blocking` — resolve repo_id from config
///   2. `.await` — fetch from HF
///   3. `spawn_blocking` — open DB, upsert pull + files, read back
pub async fn refresh_model_metadata(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    // Step 1: resolve model_id (from id_str) and repo_id (config load on blocking pool).
    let state1 = state.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let (cfg, config_dir) = load_config_from_state(&state1)?;
        let open = koji_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        let model_id = resolve_model_id(&id_str, &open.conn)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let record = koji_core::db::queries::get_model_config(&open.conn, model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let models_dir = cfg.models_dir().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        Ok::<_, (StatusCode, serde_json::Value)>((model_id, record.repo_id, config_dir, models_dir))
    })
    .await;
    let (model_id, repo_id, config_dir, _models_dir) = match resolved {
        Ok(Ok(x)) => x,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Step 2: async HF fetches (no DB handle held).
    let listing = match koji_core::models::pull::list_gguf_files(&repo_id).await {
        Ok(l) => l,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!("HuggingFace listing failed: {}", e)
                })),
            )
                .into_response();
        }
    };
    let blobs = match koji_core::models::pull::fetch_blob_metadata(&listing.repo_id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("fetch_blob_metadata failed for {}: {}", listing.repo_id, e);
            std::collections::HashMap::new()
        }
    };

    // Step 3: DB writes (blocking pool, fresh connection).
    let repo_id_for_db = repo_id.clone();
    let config_dir_for_db = config_dir.clone();
    let commit_sha = listing.commit_sha.clone();
    let files = listing.files.clone();
    let write = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let open = koji_core::db::open(&config_dir_for_db)?;
        let conn = &open.conn;
        koji_core::db::queries::upsert_model_pull(conn, model_id, &repo_id_for_db, &commit_sha)?;
        for file in &files {
            let blob = blobs.get(&file.filename);
            koji_core::db::queries::upsert_model_file(
                conn,
                model_id,
                &repo_id_for_db,
                &file.filename,
                file.quant.as_deref(),
                blob.and_then(|b| b.lfs_sha256.as_deref()),
                blob.and_then(|b| b.size),
            )?;
        }
        let files_out = koji_core::db::queries::get_model_files(conn, model_id)?;
        let pull_out = koji_core::db::queries::get_model_pull(conn, model_id)?;
        Ok((pull_out, files_out))
    })
    .await;

    match write {
        Ok(Ok((pull, files))) => {
            let files_json: Vec<_> = files.iter().map(file_record_json).collect();
            Json(serde_json::json!({
                "ok": true,
                "id": model_id,
                "repo_id": repo_id,
                "repo_commit_sha": pull.as_ref().map(|p| p.commit_sha.clone()),
                "repo_pulled_at": pull.as_ref().map(|p| p.pulled_at.clone()),
                "files": files_json,
            }))
            .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("DB write failed: {}", e)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /api/models/:id/verify — recompute SHA-256 for every tracked file of
/// this model and compare against the stored LFS hash, persisting the result.
///
/// Sequential, CPU-bound, potentially multi-minute for large GGUFs. Runs on
/// the blocking threadpool. Per-file progress events are NOT streamed here;
/// the wizard already streams them during pulls.
pub async fn verify_model_files(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    let state1 = state.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let (_cfg, config_dir) = load_config_from_state(&state1)?;
        let open = koji_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        let model_id = resolve_model_id(&id_str, &open.conn)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let record = koji_core::db::queries::get_model_config(&open.conn, model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let models_dir = _cfg.models_dir().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        Ok::<_, (StatusCode, serde_json::Value)>((model_id, record.repo_id, config_dir, models_dir))
    })
    .await;
    let (model_id, repo_id, config_dir, models_dir) = match resolved {
        Ok(Ok(x)) => x,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Model files live at <models_dir>/<repo_id>/<filename>.gguf
    let model_dir = koji_core::models::repo_path(&models_dir, &repo_id);
    let repo_id_clone = repo_id.clone();

    let task = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let open = koji_core::db::open(&config_dir)?;
        let results = koji_core::models::verify::verify_model(
            &open.conn,
            model_id,
            &repo_id_clone,
            &model_dir,
        )?;
        let files = koji_core::db::queries::get_model_files(&open.conn, model_id)?;
        Ok((results, files))
    })
    .await;

    match task {
        Ok(Ok((results, files))) => {
            let all_ok = results.iter().all(|r| r.ok != Some(false));
            let any_unknown = results.iter().any(|r| r.ok.is_none());
            let summary: Vec<_> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "filename": r.filename,
                        "ok": r.ok,
                        "error": r.error,
                    })
                })
                .collect();
            let files_json: Vec<_> = files.iter().map(file_record_json).collect();
            Json(serde_json::json!({
                "ok": all_ok,
                "any_unknown": any_unknown,
                "id": model_id,
                "repo_id": repo_id,
                "results": summary,
                "files": files_json,
            }))
            .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("verify failed: {}", e)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use koji_core::config::{ModelConfig, QuantEntry, QuantKind};
    use std::collections::BTreeMap;

    fn body_with_quants(quants: BTreeMap<String, QuantEntry>) -> ModelBody {
        ModelBody {
            backend: "llama".to_string(),
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: Some(true),
            context_length: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: Some(quants),
            modalities: None,
        }
    }

    fn existing_with_size(name: &str, file: &str, size: Option<u64>) -> ModelConfig {
        let mut quants = BTreeMap::new();
        quants.insert(
            name.to_string(),
            QuantEntry {
                file: file.to_string(),
                kind: QuantKind::Model,
                size_bytes: size,
                context_length: Some(4096),
            },
        );
        ModelConfig {
            backend: "llama".into(),
            args: vec![],
            sampling: None,
            model: Some("org/repo".into()),
            quant: Some("Q4_K_M".into()),
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            api_name: None,
            gpu_layers: None,
            quants,
            modalities: None,
            display_name: None,
        }
    }

    /// When an existing entry has a stored `size_bytes`, a PUT that tries to
    /// change it must be silently ignored — the server-side value wins.
    #[test]
    fn apply_model_body_preserves_existing_size_bytes() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", Some(1_234_567));

        let mut attacker_quants = BTreeMap::new();
        attacker_quants.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(42), // malicious / stale
                context_length: Some(8192),
            },
        );

        let result = apply_model_body(body_with_quants(attacker_quants), Some(existing));
        let q = result.quants.get("Q4_K_M").unwrap();
        assert_eq!(
            q.size_bytes,
            Some(1_234_567),
            "existing size_bytes must be preserved against client override"
        );
        assert_eq!(q.context_length, Some(8192));
    }

    /// When an existing entry has no stored size, we still accept the client
    /// value to avoid regressing fresh creates that haven't been verified yet.
    #[test]
    fn apply_model_body_accepts_client_size_when_none_stored() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", None);

        let mut incoming = BTreeMap::new();
        incoming.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(9_999),
                context_length: Some(4096),
            },
        );

        let result = apply_model_body(body_with_quants(incoming), Some(existing));
        assert_eq!(result.quants.get("Q4_K_M").unwrap().size_bytes, Some(9_999));
    }

    /// A brand-new model (no existing config) still honours whatever size the
    /// client supplies, so create flows aren't broken.
    #[test]
    fn apply_model_body_accepts_client_size_for_new_model() {
        let mut incoming = BTreeMap::new();
        incoming.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(5_000),
                context_length: None,
            },
        );

        let result = apply_model_body(body_with_quants(incoming), None);
        assert_eq!(result.quants.get("Q4_K_M").unwrap().size_bytes, Some(5_000));
    }

    /// A new quant key (not in the existing config) on an existing model still
    /// accepts the client value — preservation is per-key.
    #[test]
    fn apply_model_body_accepts_client_size_for_new_quant_key() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", Some(1_000));

        let mut incoming = BTreeMap::new();
        incoming.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(7),
                context_length: None,
            },
        );
        incoming.insert(
            "Q8_0".to_string(),
            QuantEntry {
                file: "Model-Q8_0.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(2_000),
                context_length: None,
            },
        );

        let result = apply_model_body(body_with_quants(incoming), Some(existing));
        assert_eq!(result.quants.get("Q4_K_M").unwrap().size_bytes, Some(1_000));
        assert_eq!(result.quants.get("Q8_0").unwrap().size_bytes, Some(2_000));
    }
}
