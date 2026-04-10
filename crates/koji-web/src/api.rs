use axum::extract::Path;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use crate::server::AppState;

pub mod backends;
pub mod middleware;

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

/// Load per-repo DB metadata for a model. Returns an empty `RepoDbMeta` on
/// any failure (missing config path, DB open error, repo never pulled) —
/// enrichment is best-effort and should never fail the containing request.
fn load_repo_db_meta(config_dir: &std::path::Path, repo_id: Option<&str>) -> RepoDbMeta {
    let Some(repo_id) = repo_id.filter(|s| !s.is_empty()) else {
        return RepoDbMeta::default();
    };
    let Ok(open) = koji_core::db::open(config_dir) else {
        return RepoDbMeta::default();
    };
    let mut meta = RepoDbMeta::default();
    if let Ok(Some(pull)) = koji_core::db::queries::get_model_pull(&open.conn, repo_id) {
        meta.commit_sha = Some(pull.commit_sha);
        meta.pulled_at = Some(pull.pulled_at);
    }
    if let Ok(files) = koji_core::db::queries::get_model_files(&open.conn, repo_id) {
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
    id: &str,
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
        "backend": m.backend,
        "model": m.model,
        "quant": m.quant,
        "mmproj": m.mmproj,
        "args": m.args,
        "sampling": m.sampling,
        "enabled": m.enabled,
        "context_length": m.context_length,
        "port": m.port,
        "api_name": m.api_name,
        "gpu_layers": m.gpu_layers,
        "quants": quants_json,
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
            let models: Vec<serde_json::Value> = cfg
                .models
                .iter()
                .map(|(id, m)| {
                    let meta = load_repo_db_meta(&config_dir, m.model.as_deref());
                    model_entry_json(id, m, &configs_dir, None, Some(&meta))
                })
                .collect();
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
pub async fn get_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match tokio::task::spawn_blocking(move || load_config_from_state(&state)).await {
        Ok(Ok((cfg, config_dir))) => {
            let configs_dir = config_dir.join("configs");
            let backends: Vec<String> = cfg.backends.keys().cloned().collect();
            match cfg.models.get(&id) {
                Some(m) => {
                    let meta = load_repo_db_meta(&config_dir, m.model.as_deref());
                    let mut val =
                        model_entry_json(&id, m, &configs_dir, Some(&backends), Some(&meta));
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
    // NEW:
    #[serde(default, alias = "display_name")]
    pub api_name: Option<String>,
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub quants: Option<std::collections::BTreeMap<String, koji_core::config::QuantEntry>>,
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
    Path(id): Path<String>,
    Json(body): Json<ModelBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (mut cfg, config_dir) = load_config_from_state(&state)?;
        if !cfg.models.contains_key(&id) {
            return Err((
                StatusCode::NOT_FOUND,
                serde_json::json!({"error": "Model not found"}),
            ));
        }
        let existing = cfg.models.remove(&id);
        cfg.models
            .insert(id.clone(), apply_model_body(body, existing));
        cfg.save_to(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        Ok((cfg, serde_json::json!({ "ok": true, "id": id })))
    })
    .await
    {
        Ok(Ok((cfg, val))) => {
            sync_proxy_config(&state_clone, cfg).await;
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
#[derive(serde::Deserialize)]
pub struct CreateModelBody {
    pub id: String,
    #[serde(flatten)]
    pub model: ModelBody,
}

pub async fn create_model(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateModelBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (mut cfg, config_dir) = load_config_from_state(&state)?;
        let id = body.id.trim().to_string();
        if id.is_empty() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "Model id cannot be empty"}),
            ));
        }
        if cfg.models.contains_key(&id) {
            return Err((
                StatusCode::CONFLICT,
                serde_json::json!({"error": format!("Model '{}' already exists", id)}),
            ));
        }
        cfg.models
            .insert(id.clone(), apply_model_body(body.model, None));
        cfg.save_to(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        Ok((cfg, serde_json::json!({ "ok": true, "id": id })))
    })
    .await
    {
        Ok(Ok((cfg, val))) => {
            sync_proxy_config(&state_clone, cfg).await;
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
    pub new_id: String,
}

/// POST /api/models/:id/rename — rename a model config entry.
pub async fn rename_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<RenameBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (mut cfg, config_dir) = load_config_from_state(&state)?;

        // Check source ID exists
        if !cfg.models.contains_key(&id) {
            return Err((
                StatusCode::NOT_FOUND,
                serde_json::json!({"error": "Model not found"}),
            ));
        }

        let new_id = body.new_id.trim().to_string();
        if new_id.is_empty() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "New model id cannot be empty"}),
            ));
        }

        // Check target ID doesn't exist
        if cfg.models.contains_key(&new_id) {
            return Err((
                StatusCode::CONFLICT,
                serde_json::json!({"error": format!("Model '{}' already exists", new_id)}),
            ));
        }

        // Rename the entry
        let entry = cfg.models.remove(&id).unwrap();
        cfg.models.insert(new_id.clone(), entry);
        cfg.save_to(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;

        Ok((cfg, serde_json::json!({ "ok": true, "id": new_id })))
    })
    .await
    {
        Ok(Ok((cfg, val))) => {
            sync_proxy_config(&state_clone, cfg).await;
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
    Path((id, quant_key)): Path<(String, String)>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (mut cfg, config_dir) = load_config_from_state(&state)?;

        // Find the model
        let model_config = cfg.models.get(&id).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                serde_json::json!({"error": "Model not found"}),
            )
        })?;

        // Find the quant entry
        let quant_entry = model_config
            .quants
            .get(&quant_key)
            .ok_or_else(|| (StatusCode::NOT_FOUND, serde_json::json!({"error": "Quant not found"})))?;

        // Clone the filename before we mutate
        let filename = quant_entry.file.clone();

        // Resolve file path: models_dir / repo_id / filename
        let repo_id = model_config.model.as_deref().unwrap_or("");
        let mut deleted_file = None;

        if !repo_id.is_empty() {
            if let Ok(models_dir) = cfg.models_dir() {
                let file_path = models_dir.join(repo_id).join(&filename);
                if file_path.exists() {
                    if let Err(e) = std::fs::remove_file(&file_path) {
                        tracing::warn!("Failed to delete quant file {}: {}", file_path.display(), e);
                    } else {
                        deleted_file = Some(filename.clone());
                    }
                }
            }
        }

        // Clean up DB record (best-effort)
        if !repo_id.is_empty() {
            if let Ok(open) = koji_core::db::open(&config_dir) {
                let _ = koji_core::db::queries::delete_model_file(&open.conn, repo_id, &filename);
            }
        }

        // Get mutable reference to model config
        let model = cfg.models.get_mut(&id).unwrap();

        // Clear active quant/mmproj if they referenced this quant
        if model.quant.as_deref() == Some(&quant_key) {
            model.quant = None;
        }
        if model.mmproj.as_deref() == Some(&quant_key) {
            model.mmproj = None;
        }

        // Remove the quant entry
        model.quants.remove(&quant_key);

        // Save config
        cfg.save_to(&config_dir)?;

        Ok((
            cfg,
            serde_json::json!({
                "ok": true,
                "id": id,
                "quant_key": quant_key,
                "deleted_file": deleted_file.unwrap_or(filename)
            }),
        ))
    })
    .await
    {
        Ok(Ok((cfg, val))) => {
            sync_proxy_config(&state_clone, cfg).await;
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
    Path(id): Path<String>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (mut cfg, config_dir) = load_config_from_state(&state)?;
        if cfg.models.remove(&id).is_none() {
            return Err((
                StatusCode::NOT_FOUND,
                serde_json::json!({"error": "Model not found"}),
            ));
        }
        cfg.save_to(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        Ok((cfg, serde_json::json!({ "ok": true })))
    })
    .await
    {
        Ok(Ok((cfg, val))) => {
            sync_proxy_config(&state_clone, cfg).await;
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

// ── Refresh / Verify ──────────────────────────────────────────────────────────

/// Resolve the model identified by `id` to its HF repo_id, the config
/// directory, and the models directory. Returns an error response if the
/// config can't be loaded, the id doesn't exist, or the entry has no source.
fn resolve_model_repo_id(
    state: &AppState,
    id: &str,
) -> Result<(String, std::path::PathBuf, std::path::PathBuf), (StatusCode, serde_json::Value)> {
    let (cfg, config_dir) = load_config_from_state(state)?;
    let model = cfg.models.get(id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            serde_json::json!({"error": "Model not found"}),
        )
    })?;
    let repo_id = model.model.clone().ok_or_else(|| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({"error": "Model has no `model` source set"}),
        )
    })?;
    let models_dir = cfg.models_dir().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": format!("models_dir resolution failed: {}", e)}),
        )
    })?;
    Ok((repo_id, config_dir, models_dir))
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

/// POST /api/models/:id/refresh — re-query HuggingFace for the current commit
/// SHA and per-file LFS hashes / sizes, and write them into the local DB.
///
/// Structured to keep `rusqlite::Connection` off `.await` points:
///   1. `spawn_blocking` — resolve repo_id from config
///   2. `.await` — fetch from HF
///   3. `spawn_blocking` — open DB, upsert pull + files, read back
pub async fn refresh_model_metadata(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Step 1: resolve repo_id (config load on blocking pool).
    let state1 = state.clone();
    let id1 = id.clone();
    let resolved = tokio::task::spawn_blocking(move || resolve_model_repo_id(&state1, &id1)).await;
    let (repo_id, config_dir, _models_dir) = match resolved {
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
        koji_core::db::queries::upsert_model_pull(conn, &repo_id_for_db, &commit_sha)?;
        for file in &files {
            let blob = blobs.get(&file.filename);
            koji_core::db::queries::upsert_model_file(
                conn,
                &repo_id_for_db,
                &file.filename,
                file.quant.as_deref(),
                blob.and_then(|b| b.lfs_sha256.as_deref()),
                blob.and_then(|b| b.size),
            )?;
        }
        let files_out = koji_core::db::queries::get_model_files(conn, &repo_id_for_db)?;
        let pull_out = koji_core::db::queries::get_model_pull(conn, &repo_id_for_db)?;
        Ok((pull_out, files_out))
    })
    .await;

    match write {
        Ok(Ok((pull, files))) => {
            let files_json: Vec<_> = files.iter().map(file_record_json).collect();
            Json(serde_json::json!({
                "ok": true,
                "id": id,
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
    Path(id): Path<String>,
) -> impl IntoResponse {
    let state1 = state.clone();
    let id1 = id.clone();
    let resolved = tokio::task::spawn_blocking(move || resolve_model_repo_id(&state1, &id1)).await;
    let (repo_id, config_dir, models_dir) = match resolved {
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
    let model_dir = models_dir.join(&repo_id);

    let repo_for_task = repo_id.clone();
    let task = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let open = koji_core::db::open(&config_dir)?;
        let results =
            koji_core::models::verify::verify_model(&open.conn, &repo_for_task, &model_dir)?;
        let files = koji_core::db::queries::get_model_files(&open.conn, &repo_for_task)?;
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
                "id": id,
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
            gpu_layers: None,
            quants: Some(quants),
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
