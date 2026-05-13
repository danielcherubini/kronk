use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use super::{apply_model_body, is_valid_repo_id, validate_model_body, ModelBody};
use crate::api::{load_config_from_state, trigger_proxy_reload};
use crate::server::AppState;

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

        let mgr = tama_core::models::ModelManager::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        if mgr.get_config_by_repo_id(&repo_id)
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
        let model_id = mgr.save_model_config(&repo_id, &model_config)
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
