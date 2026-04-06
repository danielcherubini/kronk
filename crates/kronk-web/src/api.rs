use axum::extract::Path;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use crate::server::AppState;

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
    let log_path = dir.join("kronk.log");
    // Use spawn_blocking for synchronous file I/O to avoid blocking the Tokio runtime.
    let log_path_clone = log_path.clone();
    let n = query.lines;
    let lines = tokio::task::spawn_blocking(move || {
        kronk_core::logging::tail_lines(&log_path_clone, n).unwrap_or_default()
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
    // Validate TOML by parsing. Note: kronk_core::config::Config has required fields
    // (e.g. `general`), so a partial TOML that omits top-level tables will fail here.
    // This is intentional — only fully valid config files are accepted.
    if let Err(e) = toml::from_str::<kronk_core::config::Config>(&body.content) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": format!("Invalid TOML: {e}")})),
        )
            .into_response();
    }
    // Use spawn_blocking for synchronous file I/O.
    match tokio::task::spawn_blocking(move || std::fs::write(&path, &body.content)).await {
        Ok(Ok(_)) => Json(serde_json::json!({ "ok": true })).into_response(),
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
) -> Result<(kronk_core::config::Config, std::path::PathBuf), (StatusCode, serde_json::Value)> {
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
    let cfg = kronk_core::config::Config::load_from(&config_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}),
        )
    })?;
    Ok((cfg, config_dir))
}

/// Build the full JSON for a model config entry, including all unified fields.
fn model_entry_json(
    id: &str,
    m: &kronk_core::config::ModelConfig,
    _configs_dir: &std::path::Path,
    backends: Option<&[String]>,
) -> serde_json::Value {
    let mut val = serde_json::json!({
        "id": id,
        "backend": m.backend,
        "model": m.model,
        "quant": m.quant,
        "args": m.args,
        "sampling": m.sampling,
        "enabled": m.enabled,
        "context_length": m.context_length,
        "port": m.port,
        "display_name": m.display_name,
        "gpu_layers": m.gpu_layers,
        "quants": m.quants,
    });

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
                .map(|(id, m)| model_entry_json(id, m, &configs_dir, None))
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
                    let mut val = model_entry_json(&id, m, &configs_dir, Some(&backends));
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
    pub args: Vec<String>,
    #[serde(default)]
    pub sampling: Option<kronk_core::profiles::SamplingParams>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub port: Option<u16>,
    // NEW:
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub quants: Option<std::collections::BTreeMap<String, kronk_core::config::QuantEntry>>,
}

fn apply_model_body(
    body: ModelBody,
    existing: Option<kronk_core::config::ModelConfig>,
) -> kronk_core::config::ModelConfig {
    let base = existing.unwrap_or_else(|| kronk_core::config::ModelConfig {
        backend: String::new(),
        args: vec![],
        sampling: None,
        model: None,
        quant: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        profile: None,
        display_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
    });

    // Handle sampling from body
    let sampling = body.sampling;

    kronk_core::config::ModelConfig {
        backend: body.backend,
        model: body.model,
        quant: body.quant,
        args: body.args,
        sampling,
        enabled: body.enabled.unwrap_or(base.enabled),
        context_length: body.context_length,
        port: body.port,
        health_check: base.health_check,
        profile: None,
        display_name: body.display_name,
        gpu_layers: body.gpu_layers,
        quants: body
            .quants
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    kronk_core::config::QuantEntry {
                        file: v.file,
                        size_bytes: v.size_bytes,
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
        Ok(serde_json::json!({ "ok": true, "id": id }))
    })
    .await
    {
        Ok(Ok(val)) => Json(val).into_response(),
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
        Ok(serde_json::json!({ "ok": true, "id": id }))
    })
    .await
    {
        Ok(Ok(val)) => (StatusCode::CREATED, Json(val)).into_response(),
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

        Ok(serde_json::json!({ "ok": true, "id": new_id }))
    })
    .await
    {
        Ok(Ok(val)) => Json(val).into_response(),
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
        Ok(serde_json::json!({ "ok": true }))
    })
    .await
    {
        Ok(Ok(val)) => Json(val).into_response(),
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
