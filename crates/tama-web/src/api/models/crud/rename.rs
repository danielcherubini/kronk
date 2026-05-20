use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::is_valid_repo_id;
use crate::api::models::resolve_model_id;
use crate::api::{load_config_from_state, trigger_proxy_reload};
use tama_core::proxy::ProxyState;

/// Body for rename endpoint.
#[derive(serde::Deserialize)]
pub struct RenameBody {
    pub new_repo_id: String,
}

/// POST /tama/v1/models/:id/rename — rename a model config entry.
pub async fn rename_model(
    State(state): State<Arc<ProxyState>>,
    Path(id_str): Path<String>,
    Json(body): Json<RenameBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();

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

        // Check source ID exists
        let model_id = resolve_model_id(&id_str, &mgr)
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
        let existing_record = mgr.get_config(model_id)
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
        if mgr.get_config_by_repo_id(&new_repo_id)
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
        let _ = mgr.save_model_config(&config_key, &model_config).map_err(
            |e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            },
        )?;

        // Clean up update_check record for old repo_id
        let _ = mgr.delete_update_check("model", &existing_record.repo_id);

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
