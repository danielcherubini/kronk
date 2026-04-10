use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use super::types::ModelResponse;
use crate::proxy::ProxyState;

/// Handle listing all configured models (Koji management API).
pub async fn handle_koji_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let models = state.build_status_response().await;
    let models_obj = models.get("models").and_then(|v| v.as_object());

    let result: Vec<serde_json::Value> = models_obj
        .into_iter()
        .flat_map(|models_obj| {
            models_obj.iter().filter_map(|(id, model)| {
                model.as_object().and_then(|model| {
                    serde_json::to_value(model).ok().map(|mut m| {
                        m["id"] = serde_json::Value::String(id.clone());
                        m
                    })
                })
            })
        })
        .collect();

    Json(serde_json::json!({
        "models": result
    }))
}

/// Handle getting a single model's state (Koji management API).
pub async fn handle_koji_get_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    // Check if already loaded (by server name or model name)
    let model_state = state.get_model_state(&model_id).await;

 if let Some(ms) = model_state {
         let owned_by = ms.backend();
         let created = match ms.load_time() {
             Some(load_time) => load_time
                 .duration_since(std::time::UNIX_EPOCH)
                 .unwrap_or(std::time::Duration::ZERO)
                 .as_secs(),
             None => 0,
         };
        return Json(serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": created,
            "owned_by": owned_by,
            "ready": ms.is_ready()
        }))
        .into_response();
    }

    // Check if it's a configured (but not loaded) model
    let config = state.config.read().await;
    for (config_name, server_cfg) in &config.models {
        if !server_cfg.enabled {
            continue;
        }
        if config_name == &model_id || server_cfg.model.as_deref() == Some(model_id.as_str()) {
            return Json(serde_json::json!({
                "id": config_name,
                "object": "model",
                "created": 0,
                "owned_by": server_cfg.backend,
                "ready": false
            }))
            .into_response();
        }
    }

    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "message": "Model not found",
                "type": "NotFoundError"
            }
        })),
    )
        .into_response()
}

/// Handle loading a model (Koji management API).
pub async fn handle_koji_load_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    // Check the model is present in config (model card is optional)
    if !state.config.read().await.models.contains_key(&model_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Model not configured",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response();
    }

    // Model card is optional — pass None if it doesn't exist on disk
    let model_card = state.get_model_card(&model_id).await;

    match state.load_model(&model_id, model_card.as_ref()).await {
        Ok(_) => {
            let model_state = state.get_model_state(&model_id).await;
            let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
            Json(ModelResponse {
                id: model_id,
                loaded,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Failed to load model: {}", e),
                    "type": "LoadModelError"
                }
            })),
        )
            .into_response(),
    }
}

/// Handle unloading a model (Koji management API).
pub async fn handle_koji_unload_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    // Get the server name for this model
    let server_name = state.get_available_server_for_model(&model_id).await;

    match server_name {
        Some(server_name) => {
            // Unload the model
            match state.unload_model(&server_name).await {
                Ok(_) => {
                    let model_state = state.get_model_state(&model_id).await;
                    let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
                    Json(ModelResponse {
                        id: model_id,
                        loaded,
                    })
                    .into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Failed to unload model: {}", e),
                            "type": "UnloadModelError"
                        }
                    })),
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Model not configured or not loaded",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response(),
    }
}
