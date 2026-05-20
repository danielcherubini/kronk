use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use super::{apply_model_body, is_valid_repo_id, validate_model_body, ModelBody};
use crate::api::{load_config_from_state, trigger_proxy_reload};
use tama_core::proxy::ProxyState;

/// POST /tama/v1/models — create a new model.
/// The body contains `repo_id` (HuggingFace repo name). Returns the auto-generated integer id.
#[derive(serde::Deserialize)]
pub struct CreateModelBody {
    pub repo_id: String,
    /// Optional HuggingFace metadata (README + API) to populate the stub.
    /// When provided, hf_* fields are merged into the model config.
    #[serde(default)]
    pub metadata: Option<tama_core::models::pull::HfModelMetadata>,
    #[serde(flatten)]
    pub model: ModelBody,
}

pub async fn create_model(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<CreateModelBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();

    // Validate repo_id: non-empty, max 256 chars, valid regex pattern
    let repo_id = body.repo_id.trim().to_string();
    if repo_id.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": "repo_id cannot be empty"})),
        )
            .into_response();
    }
    if repo_id.len() > 256 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": "repo_id must be at most 256 characters"})),
        )
            .into_response();
    }
    if !is_valid_repo_id(&repo_id) {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(serde_json::json!({"error": "repo_id contains invalid characters (only alphanumeric, dots, underscores, hyphens, and slashes are allowed)"}))).into_response();
    }

    // Validate ModelBody fields
    if let Err(e) = validate_model_body(&body.model) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": e})),
        )
            .into_response();
    }

    // Load config first (async, handles its own spawn_blocking)
    let (_, config_dir) = match load_config_from_state(&state).await {
        Ok(x) => x,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    match tokio::task::spawn_blocking(move || {
        let mgr = tama_core::models::ModelManager::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        if mgr
            .get_config_by_repo_id(&repo_id)
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
        // Merge HF metadata into model config if provided
        let model_config = if let Some(ref meta) = body.metadata {
            let mut mc = model_config;
            if mc.hf_format.is_none() {
                mc.hf_format = meta.hf_format.clone();
            }
            if mc.hf_base_model.is_none() {
                mc.hf_base_model = meta.hf_base_model.clone();
            }
            if mc.hf_pipeline_tag.is_none() {
                mc.hf_pipeline_tag = meta.hf_pipeline_tag.clone();
            }
            if mc.hf_total_params.is_none() {
                mc.hf_total_params = meta.hf_total_params.clone();
            }
            if mc.hf_active_params.is_none() {
                mc.hf_active_params = meta.hf_active_params.clone();
            }
            if mc.hf_architecture_type.is_none() {
                mc.hf_architecture_type = meta.hf_architecture_type.clone();
            }
            if mc.hf_context_length.is_none() {
                mc.hf_context_length = meta.hf_context_length;
            }
            if mc.hf_num_layers.is_none() {
                mc.hf_num_layers = meta.hf_num_layers;
            }
            if mc.hf_last_modified.is_none() {
                mc.hf_last_modified = meta.hf_last_modified.clone();
            }
            mc
        } else {
            model_config
        };
        let model_id = mgr
            .save_model_config(&repo_id, &model_config)
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
