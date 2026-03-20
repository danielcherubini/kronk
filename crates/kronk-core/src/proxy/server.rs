use crate::config::MAX_REQUEST_BODY_SIZE;
use crate::proxy::ProxyState;
use anyhow::Context;
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

fn json_error_response() -> Response {
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

pub struct ProxyServer {
    state: Arc<ProxyState>,
    idle_timeout_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ProxyServer {
    pub fn new(state: Arc<ProxyState>) -> Self {
        let handle = Self::start_idle_timeout_checker(state.clone());
        Self {
            state,
            idle_timeout_handle: Some(handle),
        }
    }

    fn start_idle_timeout_checker(state: Arc<ProxyState>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                let _ = state.check_idle_timeouts().await;
            }
        })
    }

    pub fn cancel_idle_timeout_checker(&mut self) {
        if let Some(handle) = self.idle_timeout_handle.take() {
            handle.abort();
        }
    }

    pub fn into_router(self) -> Router {
        Router::new()
            .route("/v1/chat/completions", post(handle_chat_completions))
            .route(
                "/v1/chat/completions/stream",
                post(handle_stream_chat_completions),
            )
            .route("/v1/models", get(handle_list_models))
            .route("/v1/models/:model_id", get(handle_get_model))
            .route("/health", get(handle_health))
            .route("/metrics", get(handle_metrics))
            .fallback(handle_fallback)
            .with_state(self.state.clone())
    }

    pub async fn run(self, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        info!("Starting proxy server on {}", addr);

        let app = self.into_router();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

#[axum::debug_handler]
async fn handle_chat_completions(state: State<Arc<ProxyState>>, req: Request<Body>) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => return json_error_response(),
    };

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
                    info!("Failed to load model {}: {}", model_name, e);
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

    forward_request(&state, &server_name, &parts, &body_bytes).await
}

#[axum::debug_handler]
async fn handle_stream_chat_completions(
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

    forward_request(&state, &server_name, &parts, &body_bytes).await
}

#[axum::debug_handler]
async fn handle_get_model(state: State<Arc<ProxyState>>, Path(model_id): Path<String>) -> Response {
    // Check if already loaded (by server name or model name)
    let model_state = state.get_model_state(&model_id).await;

    if let Some(ms) = model_state {
        let load_time = ms.load_time().unwrap_or(SystemTime::now());
        let owned_by = ms.backend();
        let created = load_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
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
    for (server_name, server_cfg) in &state.config.servers {
        if !server_cfg.enabled {
            continue;
        }
        let cfg_model = server_cfg.model.as_deref().unwrap_or(server_name.as_str());
        if cfg_model == model_id || server_name == &model_id {
            return Json(serde_json::json!({
                "id": cfg_model,
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

#[axum::debug_handler]
async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "kronk-proxy"
    }))
}

#[axum::debug_handler]
async fn handle_metrics(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
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
async fn handle_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let loaded_models = state.models.read().await;

    // Build a list of all configured (enabled) models, enriched with runtime state
    let mut data: Vec<serde_json::Value> = Vec::new();
    for (server_name, server_cfg) in &state.config.servers {
        if !server_cfg.enabled {
            continue;
        }
        let model_id = server_cfg.model.as_deref().unwrap_or(server_name.as_str());

        if let Some(model_state) = loaded_models.get(server_name) {
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
async fn handle_fallback() -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn forward_request(
    state: &Arc<ProxyState>,
    server_name: &str,
    parts: &axum::http::request::Parts,
    body_bytes: &[u8],
) -> Response {
    state
        .metrics
        .total_requests
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let model_state = state.get_model_state(server_name).await;
    if let Some(ms) = &model_state {
        let failures = ms
            .consecutive_failures()
            .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(0);
        if failures >= state.config.proxy.circuit_breaker_threshold {
            // Check if cooldown has elapsed
            if !ms.can_reload(state.config.proxy.circuit_breaker_cooldown_seconds) {
                info!(
                    "Circuit breaker cooldown active for server '{}' ({} failures). Waiting for cooldown.",
                    server_name, failures
                );
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Server {} is in cooldown due to repeated failures", server_name),
                            "type": "ServiceUnavailableError"
                        }
                    })),
                )
                    .into_response();
            }
            info!(
                "Circuit breaker tripped for server '{}' ({} failures). Unloading server.",
                server_name, failures
            );
            // Unload the server using PID from backend_pid
            if let Some(_pid) = ms.backend_pid() {
                let _ = state.unload_model(server_name).await;
            }
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Server {} is currently unavailable due to repeated failures", server_name),
                        "type": "ServiceUnavailableError"
                    }
                })),
            )
                .into_response();
        }
    }

    let backend_url = match state.get_backend_url(server_name).await {
        Ok(url) => url,
        Err(e) => {
            info!("Failed to get backend URL for {}: {}", server_name, e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Failed to get backend URL: {}", e),
                        "type": "BackendUrlError"
                    }
                })),
            )
                .into_response();
        }
    };

    // Combine backend_url with the request path and query
    let path_and_query = match parts.uri.path_and_query() {
        Some(pq) => pq,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": "Invalid request URI",
                        "type": "BadRequestError"
                    }
                })),
            )
                .into_response();
        }
    };

    let (path, query) = path_and_query
        .as_str()
        .split_once('?')
        .unwrap_or((path_and_query.as_str(), ""));

    let target_uri = format!("{}{}", backend_url, path);

    info!("Forwarding request to: {}", target_uri);

    let method = parts.method.clone();

    let mut headers = reqwest::header::HeaderMap::new();
    for (key, value) in &parts.headers {
        // Skip hop-by-hop headers
        if ![
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "transfer-encoding",
            "upgrade",
            "trailer",
            "host",
        ]
        .contains(&key.as_str())
            && value.to_str().is_ok()
        {
            headers.insert(key.clone(), value.clone());
        }
    }

    let mut query_string = query.to_string();
    if !query_string.is_empty() {
        query_string = format!("?{}", query_string);
    }

    match state
        .client
        .request(method, format!("{}{}", target_uri, query_string))
        .headers(headers)
        .body(body_bytes.to_vec())
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                state
                    .metrics
                    .successful_requests
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if let Some(ms) = &model_state {
                    if let Some(f) = ms.consecutive_failures() {
                        f.store(0, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            } else {
                state
                    .metrics
                    .failed_requests
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if status.is_server_error() {
                    if let Some(ms) = &model_state {
                        if let Some(f) = ms.consecutive_failures() {
                            f.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        // Set failure timestamp for cooldown
                        if ms.is_ready() {
                            let new_ts = SystemTime::now();
                            let mut models = state.models.write().await;
                            #[allow(clippy::collapsible_match)]
                            if let Some(existing) = models.get_mut(server_name) {
                                match existing {
                                    crate::proxy::ModelState::Ready {
                                        failure_timestamp, ..
                                    }
                                    | crate::proxy::ModelState::Starting {
                                        failure_timestamp,
                                        ..
                                    } => {
                                        *failure_timestamp = Some(new_ts);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }

            let mut builder = Response::builder().status(status);

            for (key, value) in response.headers().iter() {
                // Skip hop-by-hop headers in response
                if [
                    "connection",
                    "keep-alive",
                    "proxy-authenticate",
                    "proxy-authorization",
                    "te",
                    "transfer-encoding",
                    "upgrade",
                    "trailer",
                ]
                .contains(&key.as_str())
                {
                    continue;
                }
                if let Ok(v) = value.to_str() {
                    builder = builder.header(key.as_str(), v);
                }
            }

            let body = Body::from_stream(response.bytes_stream());
            builder.body(body).unwrap().into_response()
        }
        Err(e) => {
            state
                .metrics
                .failed_requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(ms) = &model_state {
                if let Some(f) = ms.consecutive_failures() {
                    f.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            info!("Failed to forward request: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Backend error: {}", e),
                        "type": "BadGatewayError"
                    }
                })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_proxy_routes_exist() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone());
        let app = server.into_router();
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Test health endpoint
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{}/health", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        // Test models endpoint
        let response = client
            .get(format!("http://{}/v1/models", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_chat_completions_route() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone());
        let app = server.into_router();
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{}/v1/chat/completions", bound_addr))
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 500); // Fails to load unknown model
    }

    #[tokio::test]
    async fn test_stream_route() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone());
        let app = server.into_router();
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{}/v1/chat/completions/stream", bound_addr))
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 500); // Fails to load unknown model
    }
}
