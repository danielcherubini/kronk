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

/// Maximum lengths for ModelBody fields.
const MAX_BACKEND: usize = 256;
const MAX_MODEL: usize = 256;
const MAX_QUANT: usize = 128;
const MAX_MMPROJ: usize = 128;
const MAX_API_NAME: usize = 128;
const MAX_DISPLAY_NAME: usize = 256;
const MAX_CACHE_TYPE: usize = 32;

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
    pub sampling: Option<tama_core::profiles::SamplingParams>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub num_parallel: Option<u32>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub api_name: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub quants: Option<std::collections::BTreeMap<String, tama_core::config::QuantEntry>>,
    #[serde(default)]
    pub modalities: Option<tama_core::config::ModelModalities>,
    #[serde(default)]
    pub kv_unified: Option<bool>,
    #[serde(default)]
    pub cache_type_k: Option<String>,
    #[serde(default)]
    pub cache_type_v: Option<String>,
}

fn apply_model_body(
    body: ModelBody,
    existing: Option<tama_core::config::ModelConfig>,
) -> tama_core::config::ModelConfig {
    let base = existing.unwrap_or_else(|| tama_core::config::ModelConfig {
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
        num_parallel: None,
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        kv_unified: true,
        cache_type_k: None,
        cache_type_v: None,
        db_id: None,
    });

    // Handle sampling from body
    let sampling = body.sampling;

    tama_core::config::ModelConfig {
        backend: body.backend,
        model: body.model,
        quant: body.quant,
        mmproj: body.mmproj,
        args: body.args,
        sampling,
        enabled: body.enabled.unwrap_or(base.enabled),
        context_length: body.context_length,
        num_parallel: body.num_parallel,
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
                    tama_core::config::QuantEntry {
                        file: v.file,
                        kind: v.kind,
                        size_bytes: preserved_size,
                        context_length: v.context_length,
                    },
                )
            })
            .collect(),
        kv_unified: body.kv_unified.unwrap_or(base.kv_unified),
        cache_type_k: body
            .cache_type_k
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        cache_type_v: body
            .cache_type_v
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        db_id: base.db_id,
    }
}

/// PUT /tama/v1/models/:id — update an existing model.
pub async fn update_model(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
    Json(body): Json<ModelBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        // Validate ModelBody fields
        if let Err(e) = validate_model_body(&body) {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": e}),
            ));
        }

        let (_cfg, config_dir) = load_config_from_state(&state)?;

        // Load existing from DB
        let open = tama_core::db::open(&config_dir).map_err(|e| {
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
        let existing_record = tama_core::db::queries::get_model_config(&open.conn, model_id)
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
        let existing = tama_core::config::ModelConfig::from_db_record(&existing_record);

        let updated_config = apply_model_body(body, Some(existing));

        // Save to DB (save_model_config converts config_key to repo_id internally)
        let config_key = existing_record.repo_id.to_lowercase().replace('/', "--");
        let new_model_id =
            tama_core::db::save_model_config(&open.conn, &config_key, &updated_config).map_err(
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

/// POST /tama/v1/models — create a new model.
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
        // Validate repo_id: non-empty, max 256 chars, valid regex pattern
        let repo_id = body.repo_id.trim().to_string();
        if repo_id.is_empty() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "repo_id cannot be empty"}),
            ));
        }
        if repo_id.len() > 256 {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "repo_id must be at most 256 characters"}),
            ));
        }
        if !is_valid_repo_id(&repo_id) {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "repo_id contains invalid characters (only alphanumeric, dots, underscores, hyphens, and slashes are allowed)"}),
            ));
        }

        // Validate ModelBody fields
        if let Err(e) = validate_model_body(&body.model) {
            return Err((StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"error": e})));
        }

        let (_, config_dir) = load_config_from_state(&state)?;

        let open = tama_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        if tama_core::db::queries::get_model_config_by_repo_id(&open.conn, &repo_id)
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
        let model_id = tama_core::db::save_model_config(&open.conn, &repo_id, &model_config)
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

/// POST /tama/v1/models/:id/rename — rename a model config entry.
pub async fn rename_model(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
    Json(body): Json<RenameBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (_, config_dir) = load_config_from_state(&state)?;

        let open = tama_core::db::open(&config_dir).map_err(|e| {
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
        let existing_record = tama_core::db::queries::get_model_config(&open.conn, model_id)
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
        let mut model_config = tama_core::config::ModelConfig::from_db_record(&existing_record);

        let new_repo_id = body.new_repo_id.trim().to_string();
        if new_repo_id.is_empty() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "New repo_id cannot be empty"}),
            ));
        }
        if new_repo_id.len() > 256 {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "New repo_id must be at most 256 characters"}),
            ));
        }
        if !is_valid_repo_id(&new_repo_id) {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({"error": "New repo_id contains invalid characters (only alphanumeric, dots, underscores, hyphens, and slashes are allowed)"}),
            ));
        }

        // Check target repo_id doesn't already exist
        if tama_core::db::queries::get_model_config_by_repo_id(&open.conn, &new_repo_id)
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
        let _ = tama_core::db::save_model_config(&open.conn, &config_key, &model_config).map_err(
            |e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            },
        )?;

        // Clean up update_check record for old repo_id
        let _ = tama_core::db::queries::delete_update_check(
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

/// DELETE /tama/v1/models/:id/quants/:quant_key — delete a single quant's file
/// and remove it from the config.
pub async fn delete_quant(
    State(state): State<Arc<AppState>>,
    Path((id, quant_key)): Path<(i64, String)>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (cfg, config_dir) = load_config_from_state(&state)?;

        let open = tama_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;

        // Find the model from DB
        let model_record = tama_core::db::queries::get_model_config(&open.conn, id)
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

        let mut model_config = tama_core::config::ModelConfig::from_db_record(&model_record);

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
        tama_core::db::save_model_config(&open.conn, &config_key, &model_config).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;

        // Clean up file (best-effort) - only after config is saved
        if !repo_id.is_empty() {
            if let Ok(models_dir) = cfg.models_dir() {
                let file_path = tama_core::models::repo_path(&models_dir, &repo_id).join(&filename);
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
            let _ = tama_core::db::queries::delete_model_file(&open.conn, id, &filename);
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

/// DELETE /tama/v1/models/:id — delete a model.
pub async fn delete_model(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    let state_clone = state.clone();
    match tokio::task::spawn_blocking(move || {
        let (cfg, config_dir) = load_config_from_state(&state)?;

        // Capture the removed model for cleanup
        let mut open = tama_core::db::open(&config_dir).map_err(|e| {
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
        let model_record = tama_core::db::queries::get_model_config(&open.conn, model_id)
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
        let _model_config = tama_core::config::ModelConfig::from_db_record(&model_record);

        // Step 1: Delete DB records within a transaction — all-or-nothing semantics.
        // This ensures that if the transaction fails, no files are touched yet
        // and the DB remains consistent.
        {
            let repo_id = model_record.repo_id.clone();

            // Start transaction
            let tx = match open.conn.transaction() {
                Ok(tx) => tx,
                Err(e) => {
                    tracing::error!("Failed to start transaction for model deletion: {e}");
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        serde_json::json!({"error": "Failed to delete model records from database"}),
                    ));
                }
            };

            // Delete the model config record — CASCADE handles model_files and model_pulls.
            tracing::debug!("Deleting model config for id={}", model_id);
            if let Err(e) =
                tama_core::db::queries::delete_model_config(&tx, model_id)
            {
                tracing::error!("Failed to delete model config: {e}");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": format!("Failed to delete model records from database: {e}")}),
                ));
            }

            // Delete update check record (best-effort, non-fatal)
            if let Err(e) =
                tama_core::db::queries::delete_update_check(&tx, "model", &repo_id)
            {
                tracing::warn!("Failed to delete update check (non-fatal): {e}");
            }

            // Commit the transaction — after this point, DB is clean.
            if let Err(e) = tx.commit() {
                tracing::error!("Failed to commit transaction for model deletion: {e}");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": "Failed to delete model records from database"}),
                ));
            }
        }

        // Step 2: File cleanup (best-effort) — after successful DB commit.
        // If file deletion fails, the DB is already clean; orphaned files are
        // a benign cleanup issue. If it had succeeded before the DB commit,
        // a failed transaction would leave files deleted but DB records intact.
        let repo_id = model_record.repo_id.clone();
        if !repo_id.is_empty() {
            // 1. Delete model directory: models_dir / repo_id
            if let Ok(models_dir) = cfg.models_dir() {
                let model_dir = tama_core::models::repo_path(&models_dir, &repo_id);
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
        }

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

// ── Validation helpers ──────────────────────────────────────────────────────

/// Validate that a string is a valid repo_id: non-empty, only alphanumeric, dots, underscores, hyphens, slashes.
fn is_valid_repo_id(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    for ch in input.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' | '/' => continue,
            _ => return false,
        }
    }
    true
}

/// Validate ModelBody field lengths. Returns an error message string if invalid.
fn validate_model_body(body: &ModelBody) -> Result<(), String> {
    if body.backend.is_empty() {
        return Err("backend cannot be empty".to_string());
    }
    if body.backend.len() > MAX_BACKEND {
        return Err(format!("backend must be at most {MAX_BACKEND} characters"));
    }
    if let Some(ref model) = body.model {
        if model.is_empty() {
            return Err("model cannot be empty".to_string());
        }
        if model.len() > MAX_MODEL {
            return Err(format!("model must be at most {MAX_MODEL} characters"));
        }
    }
    if let Some(ref quant) = body.quant {
        if !quant.is_empty() && quant.len() > MAX_QUANT {
            return Err(format!("quant must be at most {MAX_QUANT} characters"));
        }
    }
    if let Some(ref mmproj) = body.mmproj {
        if !mmproj.is_empty() && mmproj.len() > MAX_MMPROJ {
            return Err(format!("mmproj must be at most {MAX_MMPROJ} characters"));
        }
    }
    if let Some(ref api_name) = body.api_name {
        if !api_name.is_empty() && api_name.len() > MAX_API_NAME {
            return Err(format!(
                "api_name must be at most {MAX_API_NAME} characters"
            ));
        }
    }
    if let Some(ref display_name) = body.display_name {
        if !display_name.is_empty() && display_name.len() > MAX_DISPLAY_NAME {
            return Err(format!(
                "display_name must be at most {MAX_DISPLAY_NAME} characters"
            ));
        }
    }
    if let Some(ref cache_type_k) = body.cache_type_k {
        let trimmed = cache_type_k.trim();
        if !trimmed.is_empty() && trimmed.len() > MAX_CACHE_TYPE {
            return Err(format!(
                "cache_type_k must be at most {MAX_CACHE_TYPE} characters"
            ));
        }
    }
    if let Some(ref cache_type_v) = body.cache_type_v {
        let trimmed = cache_type_v.trim();
        if !trimmed.is_empty() && trimmed.len() > MAX_CACHE_TYPE {
            return Err(format!(
                "cache_type_v must be at most {MAX_CACHE_TYPE} characters"
            ));
        }
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tama_core::config::{ModelConfig, QuantEntry, QuantKind};

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
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: Some(quants),
            modalities: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
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
            num_parallel: None,
            profile: None,
            api_name: None,
            gpu_layers: None,
            quants,
            modalities: None,
            display_name: None,
            kv_unified: false,
            cache_type_k: None,
            cache_type_v: None,
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
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
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
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
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
            num_parallel: None,
            port: None,
            api_name: Some("my-api-name".to_string()),
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
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
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: Some(32),
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
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
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: Some("My Model".to_string()),
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
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
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.context_length, Some(8192));
    }

    /// Verify that num_parallel flows from body through to ModelConfig.
    #[test]
    fn test_apply_model_body_num_parallel_passthrough() {
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
            display_name: None,
            num_parallel: Some(4),
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.num_parallel, Some(4));
    }

    #[test]
    fn test_apply_model_body_num_parallel_default() {
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
            display_name: None,
            num_parallel: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.num_parallel, None);
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
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: Some(BTreeMap::new()), // empty map
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert!(result.quants.is_empty());
    }

    /// When an existing model has `kv_unified: false` and the body omits the
    /// field, the existing value must be preserved (not overwritten to true).
    #[test]
    fn test_apply_model_body_kv_unified_passthrough() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", None);
        assert!(!existing.kv_unified, "helper must create kv_unified=false");

        let body = ModelBody {
            backend: "llama".to_string(),
            model: Some("org/repo".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None, // omitted — should preserve existing
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, Some(existing));
        assert!(
            !result.kv_unified,
            "existing kv_unified=false must be preserved when body omits the field"
        );
    }

    /// When creating a new model (no existing config) and the body omits
    /// `kv_unified`, the result must default to `true`.
    #[test]
    fn test_apply_model_body_kv_unified_default_true_for_new() {
        let body = ModelBody {
            backend: "llama".to_string(),
            model: Some("org/repo".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None, // omitted — should default to true
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert!(
            result.kv_unified,
            "new model must default kv_unified to true when body omits the field"
        );
    }

    /// Verify that cache_type_k and cache_type_v flow from body through to ModelConfig.
    #[test]
    fn test_apply_model_body_cache_type_passthrough() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: Some("q4_0".to_string()),
            cache_type_v: Some("q8_0".to_string()),
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.cache_type_k, Some("q4_0".to_string()));
        assert_eq!(result.cache_type_v, Some("q8_0".to_string()));
    }

    /// cache_type_k that exceeds MAX_CACHE_TYPE must be rejected.
    #[test]
    fn test_validate_cache_type_k_too_long() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None,
            cache_type_k: Some("a".repeat(MAX_CACHE_TYPE + 1)),
            cache_type_v: None,
        };
        let result = validate_model_body(&body);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cache_type_k"));
    }

    /// cache_type_v that exceeds MAX_CACHE_TYPE must be rejected.
    #[test]
    fn test_validate_cache_type_v_too_long() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        };
        let result = validate_model_body(&body);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cache_type_v"));
    }

    /// cache_type_k/v at exactly MAX_CACHE_TYPE must pass.
    #[test]
    fn test_validate_cache_type_at_limit() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None,
            cache_type_k: Some("a".repeat(MAX_CACHE_TYPE)),
            cache_type_v: Some("b".repeat(MAX_CACHE_TYPE)),
        };
        assert!(validate_model_body(&body).is_ok());
    }

    /// When cache_type_k/v are omitted in the body, they should be None.
    #[test]
    fn test_apply_model_body_cache_type_defaults_none() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.cache_type_k, None);
        assert_eq!(result.cache_type_v, None);
    }

    /// Whitespace-only cache_type_k/v must be normalized to None.
    #[test]
    fn test_apply_model_body_cache_type_whitespace_only_becomes_none() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: Some("   ".to_string()),
            cache_type_v: Some("\t\n".to_string()),
        };

        let result = apply_model_body(body, None);
        assert_eq!(
            result.cache_type_k, None,
            "whitespace-only cache_type_k must become None"
        );
        assert_eq!(
            result.cache_type_v, None,
            "whitespace-only cache_type_v must become None"
        );
    }

    /// cache_type_k/v with leading/trailing whitespace must be trimmed.
    #[test]
    fn test_apply_model_body_cache_type_trims_whitespace() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: Some("  q4_0  ".to_string()),
            cache_type_v: Some(" q8_0 ".to_string()),
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.cache_type_k, Some("q4_0".to_string()));
        assert_eq!(result.cache_type_v, Some("q8_0".to_string()));
    }
}
