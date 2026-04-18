use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::resolve_model_id;
use crate::api::{load_config_from_state, trigger_proxy_reload};
use crate::server::AppState;

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
        db_id: None,
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
        db_id: base.db_id,
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
        let mut model_config = koji_core::config::ModelConfig::from_db_record(&existing_record);

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

        // Update the model field (repo_id) in the config to reflect the rename
        model_config.model = Some(new_repo_id.clone());

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
            db_id: None,
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

    // ── apply_model_body additional tests ─────────────────────────────────

    #[test]
    fn test_apply_model_body_preserves_existing_size() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", Some(10_000));

        let mut incoming = BTreeMap::new();
        incoming.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M-new.gguf".to_string(), // different file
                kind: QuantKind::Model,
                size_bytes: Some(5_000), // client sends smaller size
                context_length: None,
            },
        );

        let result = apply_model_body(body_with_quants(incoming), Some(existing));
        // Existing size_bytes should be preserved (server-side authoritative)
        assert_eq!(
            result.quants.get("Q4_K_M").unwrap().size_bytes,
            Some(10_000)
        );
    }

    #[test]
    fn test_apply_model_body_enabled_override() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: Some(false),
            context_length: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
        };

        let result = apply_model_body(body, None);
        assert!(!result.enabled);
    }

    #[test]
    fn test_apply_model_body_enabled_default() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None, // Not specified
            context_length: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
        };

        let result = apply_model_body(body, None);
        // Default enabled is true
        assert!(result.enabled);
    }

    #[test]
    fn test_apply_model_body_with_api_name() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            port: None,
            api_name: Some("my-api-name".to_string()),
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.api_name, Some("my-api-name".to_string()));
    }

    #[test]
    fn test_apply_model_body_with_gpu_layers() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            port: None,
            api_name: None,
            gpu_layers: Some(32),
            quants: None,
            modalities: None,
            display_name: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.gpu_layers, Some(32));
    }

    #[test]
    fn test_apply_model_body_with_display_name() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: Some("My Model".to_string()),
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.display_name, Some("My Model".to_string()));
    }

    #[test]
    fn test_apply_model_body_context_length() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: Some(8192),
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.context_length, Some(8192));
    }

    #[test]
    fn test_apply_model_body_empty_quants() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: Some(BTreeMap::new()), // empty map
            modalities: None,
            display_name: None,
        };

        let result = apply_model_body(body, None);
        assert!(result.quants.is_empty());
    }
}
