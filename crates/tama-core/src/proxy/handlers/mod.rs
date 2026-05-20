pub mod tts;

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
use std::time::Duration;
use tracing::{info, warn};

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

/// Update the last_used_model in DB. Best-effort — never fails the request.
/// Throttled: only writes if the server_name differs from what's stored.
async fn update_last_used_best_effort(state: &ProxyState, server_name: &str, model_name: &str) {
    let Some(mgr) = state.model_mgr() else {
        return;
    };
    let current = mgr.get_last_used().ok().flatten();
    if current.as_ref().map(|r| r.server_name.as_str()) == Some(server_name) {
        return; // Same model, no write needed
    }
    let _ = mgr.set_last_used(server_name, model_name);
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

    // Check for wildcard model
    if model_name == crate::proxy::WILDCARD_MODEL_NAME {
        match state.resolve_wildcard_model().await {
            Ok(server_name) => {
                state.update_last_accessed(&server_name).await;
                update_last_used_best_effort(&state, &server_name, model_name).await;
                return forward_request(
                    &state,
                    &server_name,
                    &parts,
                    &body_bytes,
                    Some(model_name),
                )
                .await;
            }
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("No model available: {}", e),
                            "type": "NoModelError"
                        }
                    })),
                )
                    .into_response();
            }
        }
    }

    let server_name = match state.get_available_server_for_model(model_name).await {
        Some(name) => name,
        None => {
            let _ = state.evict_lru_if_needed().await;
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
    update_last_used_best_effort(&state, &server_name, model_name).await;

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

    // Check for wildcard model
    if model_name == crate::proxy::WILDCARD_MODEL_NAME {
        match state.resolve_wildcard_model().await {
            Ok(server_name) => {
                state.update_last_accessed(&server_name).await;
                update_last_used_best_effort(&state, &server_name, model_name).await;
                return forward_request(
                    &state,
                    &server_name,
                    &parts,
                    &body_bytes,
                    Some(model_name),
                )
                .await;
            }
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("No model available: {}", e),
                            "type": "NoModelError"
                        }
                    })),
                )
                    .into_response();
            }
        }
    }

    let server_name = match state.get_available_server_for_model(model_name).await {
        Some(name) => name,
        None => {
            let _ = state.evict_lru_if_needed().await;
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
    update_last_used_best_effort(&state, &server_name, model_name).await;

    forward_request(&state, &server_name, &parts, &body_bytes, Some(model_name)).await
}

/// Find a matching model entry from backend responses.
/// - If entries has exactly one model → return it
/// - If multiple → try to match by config's `model` field against backend's `id` (file path)
/// - If no match → return first entry (best guess)
fn find_model_in_entries(
    entries: &[serde_json::Value],
    config_model: Option<&str>,
) -> Option<serde_json::Value> {
    if entries.is_empty() {
        return None;
    }
    if entries.len() == 1 {
        return Some(entries[0].clone());
    }
    // Multiple entries: try to match by config's model field (file path)
    if let Some(model_path) = config_model {
        for entry in entries {
            if let Some(id) = entry.get("id").and_then(|v| v.as_str()) {
                if id == model_path {
                    return Some(entry.clone());
                }
            }
        }
    }
    // No match found — return first entry as best guess
    Some(entries[0].clone())
}

#[axum::debug_handler]
pub async fn handle_get_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    // Phase 1: Look up model by model_id in config.
    // Match by config_name, api_name, or model field.
    let (config_name, server_cfg) = {
        let model_configs = state.model_configs.read().await;
        let mut found: Option<(&String, &crate::config::ModelConfig)> = None;

        for (name, cfg) in model_configs.iter() {
            if !cfg.enabled {
                continue;
            }
            if name == &model_id
                || cfg.api_name.as_deref() == Some(&*model_id)
                || cfg.model.as_deref() == Some(model_id.as_str())
            {
                found = Some((name, cfg));
                break;
            }
        }

        match found {
            Some((name, cfg)) => (name.clone(), cfg.clone()),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model not found",
                            "type": "NotFoundError"
                        }
                    })),
                )
                    .into_response();
            }
        }
    };

    // Phase 2: Check if the config's backend is loaded and Ready.
    if let Some(crate::proxy::ModelState::Ready { backend_url, .. }) =
        state.models.read().await.get(&config_name)
    {
        // Query backend's /v1/models and find matching entry
        let entries = fetch_models_from_backend(&state, backend_url).await;
        if let Some(mut entry) = find_model_in_entries(&entries, server_cfg.model.as_deref()) {
            entry["ready"] = serde_json::value::to_value(true).unwrap();
            return Json(entry).into_response();
        }
    }

    // Phase 3: Fallback — construct from config (no meta, ready: false).
    let model_id_val = server_cfg.api_name.as_deref().unwrap_or(&config_name);
    Json(serde_json::json!({
        "id": model_id_val,
        "object": "model",
        "created": 0,
        "owned_by": server_cfg.backend,
        "ready": false
    }))
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
        "service": "tama-proxy"
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
    // Phase 1: Snapshot data under locks, then drop them before I/O.
    let (backend_info, has_available_llm, all_configs) = {
        let models = state.models.read().await;
        let configs = state.model_configs.read().await;

        // Collect (config_name, backend_url, is_ready) for all models
        let backend_info: Vec<_> = models
            .iter()
            .map(|(name, ms)| {
                if let crate::proxy::ModelState::Ready { backend_url, .. } = ms {
                    (name.clone(), Some(backend_url.clone()), true)
                } else {
                    (name.clone(), None, false)
                }
            })
            .collect();

        // Clone config map for use outside lock
        let configs = configs.clone();

        // Check if any non-TTS model is Ready or Starting (for wildcard ready flag)
        let has_available_llm = models.iter().any(|(_, s)| {
            !s.is_tts_backend()
                && (s.is_ready() || matches!(s, crate::proxy::ModelState::Starting { .. }))
        });

        (backend_info, has_available_llm, configs)
    };
    // All locks dropped here

    // Phase 2: Query all Ready backends concurrently.
    let futures: Vec<_> = backend_info
        .iter()
        .filter_map(|(_, url, _)| url.as_ref().map(|u| fetch_models_from_backend(&state, u)))
        .collect();
    let results: Vec<Vec<serde_json::Value>> = futures::future::join_all(futures).await;

    // Phase 3: Merge results and inject `ready`.
    let mut data: Vec<serde_json::Value> = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // Track which config_names were served by backends (for fallback logic)
    let mut served_config_names: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // We need to correlate backend results with config_names.
    // The order of `results` matches the order of Ready backends in `backend_info`.
    let mut ready_iter = backend_info.iter().filter(|(_, _, ready)| *ready);
    for entries in results {
        if let Some((config_name, _, _)) = ready_iter.next() {
            served_config_names.insert(config_name.clone());
            for mut entry in entries {
                let id = entry
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if seen_ids.contains(&id) {
                    warn!("Duplicate model id {} from backends", id);
                    continue;
                }
                seen_ids.insert(id);

                // Inject ready
                entry["ready"] = serde_json::value::to_value(true).unwrap();
                data.push(entry);
            }
        }
    }

    // Phase 4: Add unloaded models (in config but not loaded on any backend).
    for (config_name, server_cfg) in all_configs.iter() {
        if !server_cfg.enabled {
            continue;
        }
        let model_id = server_cfg.api_name.as_deref().unwrap_or(config_name);
        if seen_ids.contains(model_id) {
            continue; // already added from backend
        }
        data.push(serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": 0,
            "owned_by": server_cfg.backend,
            "ready": false
        }));
    }

    // Phase 5: Prepend wildcard entry.
    data.insert(
        0,
        serde_json::json!({
            "id": crate::proxy::WILDCARD_MODEL_NAME,
            "object": "model",
            "created": 0,
            "owned_by": "tama-proxy",
            "ready": has_available_llm
        }),
    );

    Json(serde_json::json!({
        "object": "list",
        "data": data
    }))
}

#[axum::debug_handler]
pub async fn handle_fallback() -> StatusCode {
    StatusCode::NOT_FOUND
}

/// Wildcard POST handler: forwards all non-/tama/* requests to the backend.
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
        // Check for wildcard model
        if model.as_str() == crate::proxy::WILDCARD_MODEL_NAME {
            match state.resolve_wildcard_model().await {
                Ok(server_name) => {
                    state.update_last_accessed(&server_name).await;
                    update_last_used_best_effort(&state, &server_name, model).await;
                    return forward_request(
                        &state,
                        &server_name,
                        &parts,
                        &body_bytes,
                        Some(model.as_str()),
                    )
                    .await;
                }
                Err(e) => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(serde_json::json!({
                            "error": {
                                "message": format!("No model available: {}", e),
                                "type": "NoModelError"
                            }
                        })),
                    )
                        .into_response();
                }
            }
        }

        match state.get_available_server_for_model(model).await {
            Some(name) => name,
            None => {
                let _ = state.evict_lru_if_needed().await;
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
    if let Some(ref model) = model_name {
        update_last_used_best_effort(&state, &server_name, model).await;
    }
    forward_request(
        &state,
        &server_name,
        &parts,
        &body_bytes,
        model_name.as_deref(),
    )
    .await
}

/// Wildcard GET handler: forwards all non-/tama/* requests to the backend.
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

/// Parse a /v1/models response body and extract the `data` array.
/// Returns empty Vec if the response is invalid or missing `data`.
pub fn parse_models_response(body: &[u8]) -> Vec<serde_json::Value> {
    let parsed: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    parsed
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| arr.to_vec())
        .unwrap_or_default()
}

/// Query a single backend's /v1/models endpoint and return the `data` array.
/// Returns an empty Vec on any error (backend down, bad response, timeout).
pub async fn fetch_models_from_backend(
    state: &ProxyState,
    backend_url: &str,
) -> Vec<serde_json::Value> {
    let url = format!("{}/v1/models", backend_url);
    match state
        .client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => match response.bytes().await {
            Ok(body) => parse_models_response(&body),
            Err(e) => {
                warn!("Failed to read response body from {}: {}", backend_url, e);
                Vec::new()
            }
        },
        Err(e) => {
            warn!("Failed to fetch /v1/models from {}: {}", backend_url, e);
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ModelConfig};
    use crate::proxy::ProxyState;
    use axum::{http::StatusCode, response::IntoResponse};
    use serde_json::Value as JsonValue;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    // ── parse_models_response tests ──────────────────────────────────────────

    #[test]
    fn test_parse_models_response_valid_data() {
        let body = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "model-1", "object": "model"},
                {"id": "model-2", "object": "model"}
            ]
        });
        let result = parse_models_response(body.to_string().as_bytes());
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["id"], "model-1");
        assert_eq!(result[1]["id"], "model-2");
    }

    #[test]
    fn test_parse_models_response_invalid_json() {
        let result = parse_models_response(b"this is not json");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_models_response_missing_data_field() {
        let body = serde_json::json!({
            "object": "list"
        });
        let result = parse_models_response(body.to_string().as_bytes());
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_models_response_data_not_array() {
        let body = serde_json::json!({
            "object": "list",
            "data": "not an array"
        });
        let result = parse_models_response(body.to_string().as_bytes());
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_models_response_empty_data_array() {
        let body = serde_json::json!({
            "object": "list",
            "data": []
        });
        let result = parse_models_response(body.to_string().as_bytes());
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_models_response_empty_body() {
        let result = parse_models_response(b"");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_models_response_data_is_object() {
        let body = serde_json::json!({
            "data": {"id": "single-model"}
        });
        let result = parse_models_response(body.to_string().as_bytes());
        assert!(result.is_empty());
    }

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
        // 2 models + 1 wildcard virtual entry
        assert_eq!(data.len(), 3);

        // First entry should be the wildcard virtual entry
        assert_eq!(
            data[0].get("id").unwrap().as_str().unwrap(),
            crate::proxy::WILDCARD_MODEL_NAME
        );

        // Collect all model ids (excluding wildcard)
        let ids: Vec<&str> = data[1..]
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

    // ── Wildcard routing tests ────────────────────────────────────────────────

    /// Create a test state with a Ready model loaded.
    async fn create_test_state_with_ready_model() -> Arc<ProxyState> {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Add a model config
        {
            let mut mc = state.model_configs.write().await;
            mc.insert(
                "test-server".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: None,
                    model: Some("test/model".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        // Add a Ready model state
        {
            let mut models = state.models.write().await;
            models.insert(
                "test-server".to_string(),
                crate::proxy::ModelState::Ready {
                    model_name: "test-model".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_pid: 1234,
                    backend_url: "http://localhost:8080".to_string(),
                    load_time: std::time::SystemTime::now(),
                    last_accessed: std::time::Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    restart_count: 0,
                },
            );
        }

        Arc::new(state)
    }

    #[tokio::test]
    async fn test_handle_chat_completions_wildcard_routes_to_loaded_model() {
        let state_arc = create_test_state_with_ready_model().await;
        let state = State(state_arc.clone());

        // POST with wildcard model name
        let body = serde_json::json!({
            "model": crate::proxy::WILDCARD_MODEL_NAME,
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let req = Request::post("/v1/chat/completions")
            .body(Body::from(body.to_string().into_bytes()))
            .unwrap();

        let response = handle_chat_completions(state, req).await;

        // The response will be a forward (likely 404 from non-existent backend),
        // but it should NOT be 503 (No model available).
        // The key assertion: wildcard was resolved, not treated as unknown model.
        let status = response.status();
        // Should not be 503 (no model available) — it should attempt to forward
        assert_ne!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "Wildcard should resolve to loaded model, not return 503"
        );
    }

    #[tokio::test]
    async fn test_handle_chat_completions_wildcard_503_no_models() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);
        let state = State(state_arc.clone());

        // POST with wildcard model name but no models loaded
        let body = serde_json::json!({
            "model": crate::proxy::WILDCARD_MODEL_NAME,
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let req = Request::post("/v1/chat/completions")
            .body(Body::from(body.to_string().into_bytes()))
            .unwrap();

        let response = handle_chat_completions(state, req).await;

        // No models loaded → 503
        let status = response.status();
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "Wildcard with no models should return 503"
        );

        let (_parts, body_bytes) = response.into_response().into_parts();
        let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["error"]["type"].as_str(), Some("NoModelError"));
    }

    #[tokio::test]
    async fn test_handle_list_models_includes_wildcard() {
        let state_arc = create_test_state_with_ready_model().await;
        let state = State(state_arc.clone());

        let response = handle_list_models(state).await;
        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        let data = json.get("data").unwrap().as_array().unwrap();

        // First entry should be the wildcard virtual entry
        assert!(data.len() >= 1, "Should have at least the wildcard entry");
        assert_eq!(
            data[0].get("id").unwrap().as_str().unwrap(),
            crate::proxy::WILDCARD_MODEL_NAME
        );
        assert_eq!(data[0].get("object").unwrap().as_str(), Some("model"));
        assert_eq!(
            data[0].get("owned_by").unwrap().as_str(),
            Some("tama-proxy")
        );
        // ready should be true since we have a Ready LLM model
        assert_eq!(data[0].get("ready").unwrap().as_bool(), Some(true));
    }

    #[tokio::test]
    async fn test_handle_list_models_wildcard_ready_false_when_only_tts() {
        let config = Config::default();
        let state_inner = ProxyState::new(config, None);

        // Add only a TTS backend
        {
            let mut models = state_inner.models.write().await;
            models.insert(
                "tts_server".to_string(),
                crate::proxy::ModelState::Ready {
                    model_name: "kokoro".to_string(),
                    backend: "tts_kokoro".to_string(),
                    backend_pid: 300,
                    backend_url: "http://localhost:9000".to_string(),
                    load_time: std::time::SystemTime::now(),
                    last_accessed: std::time::Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    restart_count: 0,
                },
            );
        }

        let state_arc = Arc::new(state_inner);
        let state = State(state_arc.clone());

        let response = handle_list_models(state).await;
        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        let data = json.get("data").unwrap().as_array().unwrap();
        // Wildcard entry should have ready: false (only TTS, no LLM)
        assert_eq!(
            data[0].get("id").unwrap().as_str().unwrap(),
            crate::proxy::WILDCARD_MODEL_NAME
        );
        assert_eq!(data[0].get("ready").unwrap().as_bool(), Some(false));
    }

    // ── Integration tests: handler + DB persistence ──────────────────────────

    /// Integration test: handle_chat_completions with wildcard model and DB-backed state.
    /// Verifies the full handler → resolve_wildcard_model → DB fallback flow.
    #[tokio::test]
    async fn test_handle_chat_completions_wildcard_with_db_fallback() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = Config::default();
        let state_inner = ProxyState::new(config, Some(temp_dir.path().to_path_buf()));
        let state_arc = Arc::new(state_inner);
        let state = State(state_arc.clone());

        // No models loaded, no DB record → 503 with NoModelError
        let body = serde_json::json!({
            "model": crate::proxy::WILDCARD_MODEL_NAME,
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let req = Request::post("/v1/chat/completions")
            .body(Body::from(body.to_string().into_bytes()))
            .unwrap();

        let response = handle_chat_completions(state.clone(), req).await;
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "Should return 503 when no models and no DB record"
        );

        let (_parts, body_bytes) = response.into_response().into_parts();
        let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["type"].as_str(), Some("NoModelError"));
    }

    /// Integration test: handle_stream_chat_completions with wildcard model.
    /// Verifies streaming handler also respects wildcard routing.
    #[tokio::test]
    async fn test_handle_stream_chat_completions_wildcard_503_no_models() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);
        let state = State(state_arc.clone());

        // POST with wildcard model name but no models loaded
        let body = serde_json::json!({
            "model": crate::proxy::WILDCARD_MODEL_NAME,
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let req = Request::post("/v1/chat/completions")
            .body(Body::from(body.to_string().into_bytes()))
            .unwrap();

        let response = handle_stream_chat_completions(state, req).await;

        // No models loaded → 503
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "Stream wildcard with no models should return 503"
        );

        let (_parts, body_bytes) = response.into_response().into_parts();
        let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["error"]["type"].as_str(), Some("NoModelError"));
    }

    /// Integration test: handle_forward_post with wildcard model and Ready model.
    /// Verifies the fallback POST handler also routes wildcard correctly.
    #[tokio::test]
    async fn test_handle_forward_post_wildcard_with_ready_model() {
        let state_arc = create_test_state_with_ready_model().await;

        let body = serde_json::json!({
            "model": crate::proxy::WILDCARD_MODEL_NAME,
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let req = create_forward_post_request(&body.to_string().into_bytes());

        let response = handle_forward_post(
            Path("v1/chat/completions".to_string()),
            State(state_arc.clone()),
            req,
        )
        .await;

        // Should NOT be 503 — wildcard resolved to loaded model
        assert_ne!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "Forward POST wildcard should resolve to loaded model"
        );
    }

    // ── handle_list_models: backend merge tests ──────────────────────────────

    /// Helper: set up a ProxyState with two Ready backends and model configs.
    async fn create_state_with_two_backends(
        backend1_url: &str,
        backend2_url: &str,
    ) -> Arc<ProxyState> {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Add model configs
        {
            let mut mc = state.model_configs.write().await;
            mc.insert(
                "model-a".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("api-model-a".to_string()),
                    model: Some("test/model-a".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
            mc.insert(
                "model-b".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("api-model-b".to_string()),
                    model: Some("test/model-b".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
            // Unloaded model (enabled but no backend loaded)
            mc.insert(
                "model-c".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("api-model-c".to_string()),
                    model: Some("test/model-c".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        // Add two Ready model states
        {
            let mut models = state.models.write().await;
            models.insert(
                "model-a".to_string(),
                crate::proxy::ModelState::Ready {
                    model_name: "model-a".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_pid: 1001,
                    backend_url: backend1_url.to_string(),
                    load_time: std::time::SystemTime::now(),
                    last_accessed: std::time::Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    restart_count: 0,
                },
            );
            models.insert(
                "model-b".to_string(),
                crate::proxy::ModelState::Ready {
                    model_name: "model-b".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_pid: 1002,
                    backend_url: backend2_url.to_string(),
                    load_time: std::time::SystemTime::now(),
                    last_accessed: std::time::Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    restart_count: 0,
                },
            );
        }

        Arc::new(state)
    }

    /// Test that handle_list_models merges models from two mock backends,
    /// preserves `meta` data from backend responses, and injects `ready`.
    #[tokio::test]
    async fn test_handle_list_models_merges_backend_responses_with_meta() {
        let mock_server1 = MockServer::start().await;
        let mock_server2 = MockServer::start().await;

        // Mock backend 1: returns model with meta
        let backend1_response = serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "llama3.gguf",
                    "object": "model",
                    "created": 1700000000,
                    "owned_by": "backend1",
                    "meta": {
                        "general_name": "Llama 3",
                        "general_tags": ["llama"],
                        "architecture": "llama"
                    }
                }
            ]
        });
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&backend1_response))
            .expect(1)
            .mount(&mock_server1)
            .await;

        // Mock backend 2: returns model with meta
        let backend2_response = serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "mistral.gguf",
                    "object": "model",
                    "created": 1700000001,
                    "owned_by": "backend2",
                    "meta": {
                        "general_name": "Mistral",
                        "architecture": "mistral"
                    }
                }
            ]
        });
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&backend2_response))
            .expect(1)
            .mount(&mock_server2)
            .await;

        let state_arc =
            create_state_with_two_backends(&mock_server1.uri(), &mock_server2.uri()).await;
        let state = State(state_arc.clone());

        let response = handle_list_models(state).await;
        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        let data = json.get("data").unwrap().as_array().unwrap();

        // Should have: wildcard + 2 from backends + 3 from config = 6
        // Backend IDs (llama3.gguf, mistral.gguf) don't match config api_names,
        // so all config entries are also added.
        assert_eq!(data.len(), 6, "Expected 6 entries, got: {}", data.len());

        // Wildcard should be first
        assert_eq!(
            data[0]["id"],
            crate::proxy::WILDCARD_MODEL_NAME,
            "First entry should be wildcard"
        );
        assert_eq!(data[0]["ready"], true, "Wildcard ready should be true");

        // Collect model entries (excluding wildcard)
        let model_entries: Vec<_> = data[1..].iter().collect();

        // Find the model from backend 1
        let backend1_model = model_entries
            .iter()
            .find(|e| e["id"] == "llama3.gguf")
            .expect("llama3.gguf should be in response");

        // Verify meta is preserved from backend response
        assert!(
            backend1_model.get("meta").is_some(),
            "meta should be preserved from backend response"
        );
        assert_eq!(
            backend1_model["meta"]["general_name"], "Llama 3",
            "meta.general_name should match backend response"
        );
        assert_eq!(
            backend1_model["ready"], true,
            "Loaded model should have ready: true"
        );

        // Find the model from backend 2
        let backend2_model = model_entries
            .iter()
            .find(|e| e["id"] == "mistral.gguf")
            .expect("mistral.gguf should be in response");

        assert!(
            backend2_model.get("meta").is_some(),
            "meta should be preserved from backend response"
        );
        assert_eq!(
            backend2_model["ready"], true,
            "Loaded model should have ready: true"
        );

        // Find the unloaded model (from config)
        let unloaded_model = model_entries
            .iter()
            .find(|e| e["id"] == "api-model-c")
            .expect("api-model-c should be in response as unloaded");
        assert_eq!(
            unloaded_model["ready"], false,
            "Unloaded model should have ready: false"
        );
        assert!(
            unloaded_model.get("meta").is_none(),
            "Unloaded model should not have meta"
        );
    }

    /// Test that unloaded models (in config but not loaded on any backend)
    /// still appear with ready: false and no meta.
    #[tokio::test]
    async fn test_handle_list_models_unloaded_from_config() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Add model configs — all enabled, none loaded
        {
            let mut mc = state.model_configs.write().await;
            mc.insert(
                "unloaded-model".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("my-unloaded-model".to_string()),
                    model: Some("test/unloaded".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
            // Disabled model should NOT appear
            mc.insert(
                "disabled-model".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("disabled-model".to_string()),
                    model: Some("test/disabled".to_string()),
                    enabled: false,
                    ..Default::default()
                },
            );
        }

        // No models loaded
        let state_arc = Arc::new(state);
        let state = State(state_arc.clone());

        let response = handle_list_models(state).await;
        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        let data = json.get("data").unwrap().as_array().unwrap();

        // Should have: wildcard + 1 unloaded = 2 (disabled excluded)
        assert_eq!(data.len(), 2, "Expected 2 entries, got: {}", data.len());

        // Wildcard first
        assert_eq!(data[0]["id"], crate::proxy::WILDCARD_MODEL_NAME);
        assert_eq!(
            data[0]["ready"], false,
            "Wildcard ready should be false (no LLM backends)"
        );

        // Unloaded model
        assert_eq!(data[1]["id"], "my-unloaded-model");
        assert_eq!(data[1]["ready"], false);
        assert!(data[1].get("meta").is_none());
    }

    /// Test that wildcard entry is always prepended and has correct ready value.
    #[tokio::test]
    async fn test_handle_list_models_wildcard_prepended_with_correct_ready() {
        let mock_server = MockServer::start().await;

        let backend_response = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "some-model", "object": "model", "created": 123}
            ]
        });
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&backend_response))
            .mount(&mock_server)
            .await;

        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Add one Ready model
        {
            let mut mc = state.model_configs.write().await;
            mc.insert(
                "server1".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("api-s1".to_string()),
                    model: Some("test/s1".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }
        {
            let mut models = state.models.write().await;
            models.insert(
                "server1".to_string(),
                crate::proxy::ModelState::Ready {
                    model_name: "server1".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_pid: 999,
                    backend_url: mock_server.uri(),
                    load_time: std::time::SystemTime::now(),
                    last_accessed: std::time::Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    restart_count: 0,
                },
            );
        }

        let state_arc = Arc::new(state);
        let state = State(state_arc.clone());

        let response = handle_list_models(state).await;
        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        let data = json.get("data").unwrap().as_array().unwrap();

        // Wildcard must be first
        assert_eq!(data[0]["id"], crate::proxy::WILDCARD_MODEL_NAME);
        assert_eq!(data[0]["object"], "model");
        assert_eq!(data[0]["owned_by"], "tama-proxy");
        assert_eq!(
            data[0]["ready"], true,
            "Wildcard ready should be true when LLM backend is Ready"
        );
    }

    /// Test that duplicate model IDs across backends are deduplicated.
    #[tokio::test]
    async fn test_handle_list_models_deduplicates_model_ids() {
        let mock_server1 = MockServer::start().await;
        let mock_server2 = MockServer::start().await;

        // Both backends return the same model id
        let same_response = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "duplicate-model", "object": "model", "created": 100}
            ]
        });
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&same_response))
            .mount(&mock_server1)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&same_response))
            .mount(&mock_server2)
            .await;

        let state_arc =
            create_state_with_two_backends(&mock_server1.uri(), &mock_server2.uri()).await;
        let state = State(state_arc.clone());

        let response = handle_list_models(state).await;
        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        let data = json.get("data").unwrap().as_array().unwrap();

        // Count occurrences of "duplicate-model" (excluding wildcard)
        let dup_count = data[1..]
            .iter()
            .filter(|e| e["id"] == "duplicate-model")
            .count();
        assert_eq!(
            dup_count, 1,
            "duplicate-model should appear exactly once, found {} times",
            dup_count
        );
    }

    /// Test that backend failure falls back to config-based entry.
    #[tokio::test]
    async fn test_handle_list_models_backend_failure_fallback() {
        // Don't mount any mock — the backend URL will be unreachable
        let state_arc = create_state_with_two_backends(
            "http://localhost:59999", // unreachable
            "http://localhost:59998", // unreachable
        )
        .await;
        let state = State(state_arc.clone());

        let response = handle_list_models(state).await;
        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        let data = json.get("data").unwrap().as_array().unwrap();

        // Should still have entries from config fallback
        // wildcard + model-a + model-b + model-c = 4
        assert_eq!(data.len(), 4, "Expected 4 entries from config fallback");

        // Wildcard should have ready: true (backends are in Ready state even if unreachable)
        assert_eq!(data[0]["id"], crate::proxy::WILDCARD_MODEL_NAME);
        assert_eq!(data[0]["ready"], true);
    }

    /// Test response shape matches OpenAI spec.
    #[tokio::test]
    async fn test_handle_list_models_response_shape() {
        let config = Config::default();
        let state = ProxyState::new(config, None);
        let state_arc = Arc::new(state);
        let state = State(state_arc.clone());

        let response = handle_list_models(state).await;
        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        // Must have "object": "list" at top level
        assert_eq!(json["object"], "list");
        assert!(json["data"].is_array());
    }

    // ── handle_get_model: backend fetch tests ──────────────────────────────

    /// Test that handle_get_model fetches from backend when model is loaded,
    /// preserves `meta` data, and injects `ready: true`.
    #[tokio::test]
    async fn test_handle_get_model_fetches_from_backend_with_meta() {
        let mock_server = MockServer::start().await;

        // Mock backend returns model with meta
        let backend_response = serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "llama3.gguf",
                    "object": "model",
                    "created": 1700000000,
                    "owned_by": "backend1",
                    "meta": {
                        "general_name": "Llama 3",
                        "architecture": "llama"
                    }
                }
            ]
        });
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&backend_response))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Add model config
        {
            let mut mc = state.model_configs.write().await;
            mc.insert(
                "test-model".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("my-api-model".to_string()),
                    model: Some("llama3.gguf".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        // Add a Ready model state
        {
            let mut models = state.models.write().await;
            models.insert(
                "test-model".to_string(),
                crate::proxy::ModelState::Ready {
                    model_name: "test-model".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_pid: 1234,
                    backend_url: mock_server.uri(),
                    load_time: std::time::SystemTime::now(),
                    last_accessed: std::time::Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    restart_count: 0,
                },
            );
        }

        let state_arc = Arc::new(state);
        let state = State(state_arc.clone());

        // Query by config key
        let response = handle_get_model(state.clone(), Path("test-model".to_string())).await;
        assert_eq!(response.status(), StatusCode::OK);

        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        // Should have meta from backend
        assert!(
            json.get("meta").is_some(),
            "meta should be preserved from backend response"
        );
        assert_eq!(
            json["meta"]["general_name"], "Llama 3",
            "meta.general_name should match backend response"
        );
        // ready should be injected as true
        assert_eq!(json["ready"], true, "Loaded model should have ready: true");
    }

    /// Test that handle_get_model falls back to config when model is not loaded.
    /// Response should have no `meta` and `ready: false`.
    #[tokio::test]
    async fn test_handle_get_model_fallback_to_config_when_not_loaded() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        // Add model config but do NOT add it to loaded models
        {
            let mut mc = state_arc.model_configs.write().await;
            mc.insert(
                "unloaded-model".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("my-unloaded-model".to_string()),
                    model: Some("test/unloaded".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        let state = State(state_arc.clone());

        // Query by config key
        let response = handle_get_model(state.clone(), Path("unloaded-model".to_string())).await;
        assert_eq!(response.status(), StatusCode::OK);

        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        // Should use api_name as id
        assert_eq!(json["id"], "my-unloaded-model");
        // Should NOT have meta
        assert!(
            json.get("meta").is_none(),
            "Unloaded model should not have meta"
        );
        // ready should be false
        assert_eq!(
            json["ready"], false,
            "Unloaded model should have ready: false"
        );
    }

    /// Test that handle_get_model returns 404 for unknown model IDs.
    #[tokio::test]
    async fn test_handle_get_model_404_for_unknown_model() {
        let state_inner = create_test_state();
        let state_arc = Arc::new(state_inner);

        let state = State(state_arc.clone());

        // Query with a model_id that doesn't exist in config
        let response =
            handle_get_model(state.clone(), Path("totally-unknown-model".to_string())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["error"]["type"], "NotFoundError");
    }

    /// Test that handle_get_model works when backend returns multiple models
    /// and matches by config's model field (file path).
    #[tokio::test]
    async fn test_handle_get_model_matches_by_model_field_when_multiple() {
        let mock_server = MockServer::start().await;

        // Backend returns multiple models
        let backend_response = serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "/path/to/model-a.gguf",
                    "object": "model",
                    "created": 1700000000,
                    "owned_by": "backend1"
                },
                {
                    "id": "/path/to/model-b.gguf",
                    "object": "model",
                    "created": 1700000001,
                    "owned_by": "backend1"
                }
            ]
        });
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&backend_response))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Config's model field matches model-b
        {
            let mut mc = state.model_configs.write().await;
            mc.insert(
                "my-model".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("my-api-name".to_string()),
                    model: Some("/path/to/model-b.gguf".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        {
            let mut models = state.models.write().await;
            models.insert(
                "my-model".to_string(),
                crate::proxy::ModelState::Ready {
                    model_name: "my-model".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_pid: 5678,
                    backend_url: mock_server.uri(),
                    load_time: std::time::SystemTime::now(),
                    last_accessed: std::time::Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    restart_count: 0,
                },
            );
        }

        let state_arc = Arc::new(state);
        let state = State(state_arc.clone());

        let response = handle_get_model(state.clone(), Path("my-model".to_string())).await;
        assert_eq!(response.status(), StatusCode::OK);

        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        // Should match model-b by config's model field
        assert_eq!(
            json["id"], "/path/to/model-b.gguf",
            "Should match by config's model field"
        );
        assert_eq!(json["ready"], true);
    }

    /// Test that handle_get_model falls back to config when backend query fails.
    #[tokio::test]
    async fn test_handle_get_model_backend_failure_fallback() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Add model config
        {
            let mut mc = state.model_configs.write().await;
            mc.insert(
                "fail-model".to_string(),
                ModelConfig {
                    backend: "llama_cpp".to_string(),
                    api_name: Some("fail-api".to_string()),
                    model: Some("test/fail".to_string()),
                    enabled: true,
                    ..Default::default()
                },
            );
        }

        // Add a Ready model with unreachable backend URL
        {
            let mut models = state.models.write().await;
            models.insert(
                "fail-model".to_string(),
                crate::proxy::ModelState::Ready {
                    model_name: "fail-model".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_pid: 9999,
                    backend_url: "http://localhost:59999".to_string(),
                    load_time: std::time::SystemTime::now(),
                    last_accessed: std::time::Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    restart_count: 0,
                },
            );
        }

        let state_arc = Arc::new(state);
        let state = State(state_arc.clone());

        let response = handle_get_model(state.clone(), Path("fail-model".to_string())).await;
        assert_eq!(response.status(), StatusCode::OK);

        let (_parts, body) = response.into_response().into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

        // Should fall back to config-based response
        assert_eq!(json["id"], "fail-api");
        assert!(json.get("meta").is_none());
        assert_eq!(json["ready"], false);
    }
}
