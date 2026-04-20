use crate::config::MAX_REQUEST_BODY_SIZE;
use crate::proxy::ProxyState;
use anyhow::Context;
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

use super::forward::forward_request;

pub fn json_error_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": {
                "message": "Bad Request",
                "type": "BadRequestError"
            }
        })),
    )
        .into_response()
}

#[axum::debug_handler]
pub async fn handle_chat_completions(
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let (mut parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => return json_error_response(),
    };

    // Normalise: clients that set base_url=http://host/v1 may POST to /v1 directly.
    // Rewrite to /v1/chat/completions so the backend gets the right path.
    if parts.uri.path() == "/v1" {
        if let Ok(uri) = "/v1/chat/completions".parse::<axum::http::Uri>() {
            parts.uri = uri;
        }
    }

    let request: serde_json::Value =
        match serde_json::from_slice(&body_bytes).context("Failed to parse request body") {
            Ok(r) => r,
            Err(_) => {
                return json_error_response();
            }
        };

    let model_name = match request.get("model").and_then(|v| v.as_str()) {
        Some(name) => name,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": "Missing required field: model",
                        "type": "BadRequestError"
                    }
                })),
            )
                .into_response();
        }
    };

    info!("Routing request for model: {}", model_name);

    let server_name = match state.get_available_server_for_model(model_name).await {
        Some(name) => name,
        None => {
            let model_card = state.get_model_card(model_name).await;
            match state.load_model(model_name, model_card.as_ref()).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to load model {}: {}", model_name, e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": {
                                "message": format!("Failed to load model: {}", e),
                                "type": "LoadModelError"
                            }
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    state.update_last_accessed(&server_name).await;

    forward_request(&state, &server_name, &parts, &body_bytes, Some(model_name)).await
}

#[axum::debug_handler]
pub async fn handle_stream_chat_completions(
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => return json_error_response(),
    };

    let request: serde_json::Value =
        match serde_json::from_slice(&body_bytes).context("Failed to parse request body") {
            Ok(r) => r,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Bad Request",
                            "type": "BadRequestError"
                        }
                    })),
                )
                    .into_response();
            }
        };

    let model_name = match request.get("model").and_then(|v| v.as_str()) {
        Some(name) => name,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": "Missing required field: model",
                        "type": "BadRequestError"
                    }
                })),
            )
                .into_response();
        }
    };

    info!("Streaming request for model: {}", model_name);

    let server_name = match state.get_available_server_for_model(model_name).await {
        Some(name) => name,
        None => {
            let model_card = state.get_model_card(model_name).await;
            match state.load_model(model_name, model_card.as_ref()).await {
                Ok(s) => s,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": {
                                "message": format!("Failed to load model: {}", e),
                                "type": "LoadModelError"
                            }
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    state.update_last_accessed(&server_name).await;

    forward_request(&state, &server_name, &parts, &body_bytes, Some(model_name)).await
}

#[axum::debug_handler]
pub async fn handle_get_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    // Acquire both locks upfront
    let _config = state.config.read().await;
    let model_configs = state.model_configs.read().await;
    let loaded_models = state.models.read().await;

    // First check: runtime state found by config key
    if let Some(ms) = loaded_models.get(&model_id) {
        let load_time = ms.load_time().unwrap_or(SystemTime::now());
        let owned_by = ms.backend();
        let created = load_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        // Look up config to get api_name
        if let Some(server_cfg) = model_configs.get(&model_id) {
            let model_id_val = server_cfg.api_name.as_deref().unwrap_or(&model_id);
            return Json(serde_json::json!({
                "id": model_id_val,
                "object": "model",
                "created": created,
                "owned_by": owned_by,
                "ready": ms.is_ready()
            }))
            .into_response();
        }
    }

    // Fallback: check if model_id matches config_name, api_name, or model field
    for (config_name, server_cfg) in model_configs.iter() {
        if !server_cfg.enabled {
            continue;
        }
        // Check if model_id matches config_name, api_name, or model field
        if config_name == &model_id
            || server_cfg.api_name.as_deref() == Some(&*model_id)
            || server_cfg.model.as_deref() == Some(model_id.as_str())
        {
            let model_id_val = server_cfg.api_name.as_deref().unwrap_or(config_name);
            // Check runtime state for accurate ready status
            let ready = loaded_models
                .get(config_name)
                .map(|ms| ms.is_ready())
                .unwrap_or(false);
            return Json(serde_json::json!({
                "id": model_id_val,
                "object": "model",
                "created": 0,
                "owned_by": server_cfg.backend,
                "ready": ready
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

#[axum::debug_handler]
pub async fn handle_status(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let response = state.build_status_response().await;
    Json(response)
}

#[axum::debug_handler]
pub async fn handle_reload_configs(state: State<Arc<ProxyState>>) -> impl IntoResponse {
    match state.reload_model_configs().await {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[axum::debug_handler]
pub async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "koji-proxy"
    }))
}

#[axum::debug_handler]
pub async fn handle_metrics(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let metrics = &state.metrics;
    Json(serde_json::json!({
        "total_requests": metrics.total_requests.load(std::sync::atomic::Ordering::Relaxed),
        "successful_requests": metrics.successful_requests.load(std::sync::atomic::Ordering::Relaxed),
        "failed_requests": metrics.failed_requests.load(std::sync::atomic::Ordering::Relaxed),
        "models_loaded": metrics.models_loaded.load(std::sync::atomic::Ordering::Relaxed),
        "models_unloaded": metrics.models_unloaded.load(std::sync::atomic::Ordering::Relaxed),
        "active_models": state.models.read().await.len(),
    }))
}

#[axum::debug_handler]
pub async fn handle_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let loaded_models = state.models.read().await;
    let model_configs = state.model_configs.read().await;
    let _config = state.config.read().await;

    // Build a list of all configured (enabled) models, enriched with runtime state
    let mut data: Vec<serde_json::Value> = Vec::new();
    for (config_name, server_cfg) in model_configs.iter() {
        if !server_cfg.enabled {
            continue;
        }

        let model_id = server_cfg.api_name.as_deref().unwrap_or(config_name);

        if let Some(model_state) = loaded_models.get(config_name) {
            let created = model_state
                .load_time()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            data.push(serde_json::json!({
                "id": model_id,
                "object": "model",
                "created": created,
                "owned_by": model_state.backend(),
                "ready": model_state.is_ready()
            }));
        } else {
            data.push(serde_json::json!({
                "id": model_id,
                "object": "model",
                "created": 0,
                "owned_by": server_cfg.backend,
                "ready": false
            }));
        }
    }

    Json(serde_json::json!({
        "object": "list",
        "data": data
    }))
}

#[axum::debug_handler]
pub async fn handle_fallback() -> StatusCode {
    StatusCode::NOT_FOUND
}

/// Wildcard POST handler: forwards all non-/koji/* requests to the backend.
/// Extracts `model` from the request body for auto-loading support.
#[axum::debug_handler]
pub async fn handle_forward_post(
    Path(_path): Path<String>,
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": "Request body too large",
                        "type": "BadRequestError"
                    }
                })),
            )
                .into_response()
        }
    };

    // Try to extract model for auto-loading
    let model_name: Option<String> = serde_json::from_slice::<JsonValue>(&body_bytes)
        .ok()
        .and_then(|v| v.get("model")?.as_str().map(String::from));

    let server_name = if let Some(ref model) = model_name {
        match state.get_available_server_for_model(model).await {
            Some(name) => name,
            None => {
                let card = state.get_model_card(model).await;
                match state.load_model(model, card.as_ref()).await {
                    Ok(s) => s,
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": {
                                    "message": format!("Failed to load model: {}", e),
                                    "type": "LoadModelError"
                                }
                            })),
                        )
                            .into_response()
                    }
                }
            }
        }
    } else {
        // No model field — forward to first available server or return error
        let models = state.models.read().await;
        if let Some(name) = models.keys().next().cloned() {
            drop(models);
            name
        } else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": {
                        "message": "No backend server available",
                        "type": "ServiceUnavailableError"
                    }
                })),
            )
                .into_response();
        }
    };

    state.update_last_accessed(&server_name).await;
    forward_request(
        &state,
        &server_name,
        &parts,
        &body_bytes,
        model_name.as_deref(),
    )
    .await
}

/// Wildcard GET handler: forwards all non-/koji/* requests to the backend.
#[axum::debug_handler]
pub async fn handle_forward_get(
    Path(_path): Path<String>,
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = to_bytes(body, MAX_REQUEST_BODY_SIZE)
        .await
        .unwrap_or_default();

    // GET requests don't have a model field — forward to any available server
    let models = state.models.read().await;
    let server_name = models.keys().next().cloned().unwrap_or_else(String::new);
    drop(models);

    if server_name.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": {
                    "message": "No backend server available",
                    "type": "ServiceUnavailableError"
                }
            })),
        )
            .into_response();
    }

    forward_request(&state, &server_name, &parts, &body_bytes, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ModelConfig};
    use crate::proxy::ProxyState;
    use axum::{http::StatusCode, response::IntoResponse};
    use serde_json::Value as JsonValue;

    fn create_test_state() -> ProxyState {
        let config = Config::default();
        ProxyState::new(config, None)
    }

    #[tokio::test]
    async fn test_handle_list_models_returns_api_name() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        // Populate model_configs
        {
            let mut mc = state_arc.model_configs.write().await;
            mc.insert(
                "config-key-1".to_string(),
                ModelConfig {
                    backend: "llama.cpp".to_string(),
                    api_name: Some("api-name-1".to_string()),
                    model: Some("test/model-1".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
            mc.insert(
                "config-key-2".to_string(),
                ModelConfig {
                    backend: "llama.cpp".to_string(),
                    api_name: None,
                    model: Some("test/model-2".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        let state = State(state_arc.clone());

        let response = handle_list_models(state).await;
        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        let data = json.get("data").unwrap().as_array().unwrap();
        assert_eq!(data.len(), 2);

        // Collect all model ids
        let ids: Vec<&str> = data
            .iter()
            .map(|m| m.get("id").unwrap().as_str().unwrap())
            .collect();

        // Verify all expected ids are present
        assert!(
            ids.contains(&"api-name-1"),
            "Expected 'api-name-1' in model ids, got: {:?}",
            ids
        );
        assert!(
            ids.contains(&"config-key-2"),
            "Expected 'config-key-2' in model ids, got: {:?}",
            ids
        );
    }

    #[tokio::test]
    async fn test_handle_get_model_by_config_key_returns_api_name() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        // Populate model_configs
        {
            let mut mc = state_arc.model_configs.write().await;
            mc.insert(
                "config-key-1".to_string(),
                ModelConfig {
                    backend: "llama.cpp".to_string(),
                    api_name: Some("api-name-1".to_string()),
                    model: Some("test/model-1".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        let state = State(state_arc);

        let response = handle_get_model(state, Path("config-key-1".to_string())).await;
        let status = response.status();
        assert_eq!(status, StatusCode::OK);

        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json.get("id").unwrap().as_str(), Some("api-name-1"));
    }

    #[tokio::test]
    async fn test_handle_get_model_by_api_name_returns_api_name() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        // Populate model_configs
        {
            let mut mc = state_arc.model_configs.write().await;
            mc.insert(
                "config-key-1".to_string(),
                ModelConfig {
                    backend: "llama.cpp".to_string(),
                    api_name: Some("api-name-1".to_string()),
                    model: Some("test/model-1".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        let state = State(state_arc);

        let response = handle_get_model(state, Path("api-name-1".to_string())).await;
        let status = response.status();
        assert_eq!(status, StatusCode::OK);

        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json.get("id").unwrap().as_str(), Some("api-name-1"));
    }

    #[tokio::test]
    async fn test_handle_get_model_without_api_name_falls_back_to_config_key() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        // Populate model_configs
        {
            let mut mc = state_arc.model_configs.write().await;
            mc.insert(
                "config-key-2".to_string(),
                ModelConfig {
                    backend: "llama.cpp".to_string(),
                    api_name: None,
                    model: Some("test/model-2".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        let state = State(state_arc);

        let response = handle_get_model(state, Path("config-key-2".to_string())).await;
        let status = response.status();
        assert_eq!(status, StatusCode::OK);

        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json.get("id").unwrap().as_str(), Some("config-key-2"));
    }

    // ── handle_forward_post tests ───────────────────────────────────────────

    fn create_forward_post_request(body: &[u8]) -> Request<Body> {
        Request::post("/v1/chat/completions")
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    #[tokio::test]
    async fn test_handle_forward_post_model_extraction_from_json_body() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        let body = serde_json::json!({
            "model": "my-test-model",
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let req = create_forward_post_request(&body.to_string().into_bytes());

        let response = handle_forward_post(
            Path("v1/chat/completions".to_string()),
            State(state_arc.clone()),
            req,
        )
        .await;

        // Since no model is loaded and load_model will fail, expect an error response.
        // The key assertion: the handler DID extract the model name and attempted loading.
        let status = response.status();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

        let (_parts, body_bytes) = response.into_response().into_parts();
        let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        let error_type = json["error"]["type"].as_str().unwrap();
        assert_eq!(error_type, "LoadModelError");
    }

    #[tokio::test]
    async fn test_handle_forward_post_non_json_body_does_not_crash() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        // Non-JSON body — model extraction should yield None
        let req = create_forward_post_request(b"not a json body at all");

        let response = handle_forward_post(
            Path("v1/chat/completions".to_string()),
            State(state_arc.clone()),
            req,
        )
        .await;

        // No servers available and no model field → 503
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let (_parts, body_bytes) = response.into_response().into_parts();
        let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            json["error"]["type"].as_str(),
            Some("ServiceUnavailableError")
        );
    }

    #[tokio::test]
    async fn test_handle_forward_post_missing_model_no_servers_returns_503() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        // Empty body — no model field, no servers → 503
        let req = create_forward_post_request(b"");

        let response = handle_forward_post(
            Path("v1/chat/completions".to_string()),
            State(state_arc.clone()),
            req,
        )
        .await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let (_parts, body_bytes) = response.into_response().into_parts();
        let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            json["error"]["message"].as_str(),
            Some("No backend server available")
        );
    }

    #[tokio::test]
    async fn test_handle_forward_post_load_model_error_returns_500() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        // Request with a model that doesn't exist in config — load_model will fail
        let body = serde_json::json!({
            "model": "nonexistent-model-xyz",
            "messages": [{"role": "user", "content": "Test"}]
        });
        let req = create_forward_post_request(&body.to_string().into_bytes());

        let response = handle_forward_post(
            Path("v1/chat/completions".to_string()),
            State(state_arc.clone()),
            req,
        )
        .await;

        // load_model fails because the model isn't in config → 500
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let (_parts, body_bytes) = response.into_response().into_parts();
        let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["error"]["type"].as_str(), Some("LoadModelError"));
    }

    // ── handle_forward_get tests ──────────────────────────────────────────────

    fn create_forward_get_request() -> Request<Body> {
        Request::get("/v1/models").body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn test_handle_forward_get_no_servers_returns_503() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        let req = create_forward_get_request();

        let response =
            handle_forward_get(Path("v1/models".to_string()), State(state_arc.clone()), req).await;

        // No models loaded → 503
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let (_parts, body_bytes) = response.into_response().into_parts();
        let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            json["error"]["message"].as_str(),
            Some("No backend server available")
        );
    }

    #[tokio::test]
    async fn test_handle_forward_get_empty_body_does_not_crash() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        // GET with empty body — should not panic or crash
        let req = create_forward_get_request();

        let response =
            handle_forward_get(Path("v1/models".to_string()), State(state_arc.clone()), req).await;

        // With no servers, returns 503 (not a crash)
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
