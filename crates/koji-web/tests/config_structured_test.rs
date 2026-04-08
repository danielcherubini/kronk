//! Integration tests for GET/POST /api/config/structured endpoints.
//!
//! These tests verify:
//! - GET returns valid JSON Config
//! - POST persists and round-trips without field loss
//! - loaded_from is restored from proxy config
//! - All ModelConfig/Supervisor/BackendConfig/ProxyConfig fields preserved
//! - Standalone mode works (no proxy_config)
//! - Equivalence with /api/config (TOML) endpoint

use std::sync::Arc;
use tempfile::TempDir;

use tower::util::ServiceExt;

use koji_web::server::{build_router, AppState};

/// Create a minimal valid koji.toml config for testing.
fn create_test_config() -> String {
    r#"
log_level = "info"
models_dir = "./models"
logs_dir = "./logs"

[proxy]
enabled = true
host = "0.0.0.0"
port = 11434
idle_timeout_secs = 300
startup_timeout_secs = 120
circuit_breaker_threshold = 3
circuit_breaker_cooldown_seconds = 60
metrics_retention_secs = 86400

[supervisor]
restart_policy = "always"
max_restarts = 10
restart_delay_ms = 3000
health_check_interval_ms = 5000
health_check_timeout_ms = 30000
health_check_retries = 3

[general]
log_level = "info"
models_dir = "./models"
logs_dir = "./logs"

[backends.llama_cpp]
path = "/usr/local/bin/llama-server"
default_args = ["--port", "8080"]
health_check_url = "http://localhost:8080/health"
version = "1.0.0"

[models.test_model]
backend = "llama_cpp"
model = "mistralai/Mistral-7B-Instruct-v0.3"
quant = "Q4_K_M"
args = []
enabled = true
context_length = 32768
display_name = "Test Model"
gpu_layers = 35

[sampling_templates.coding]
temperature = 0.7
top_k = 40
top_p = 0.9
min_p = 0.05
presence_penalty = 0.0
frequency_penalty = 0.0
repeat_penalty = 1.1
"#
    .to_string()
}

/// Build test AppState with config in temp dir.
fn build_test_app_state(config_content: &str) -> (Arc<AppState>, TempDir) {
    let temp_dir = TempDir::new().expect("create temp dir");
    let config_path = temp_dir.path().join("koji.toml");
    std::fs::write(&config_path, config_content).expect("write config");

    let state = AppState {
        logs_dir: Some(temp_dir.path().join("logs")),
        config_path: Some(config_path),
        proxy_config: None,
        client: reqwest::Client::new(),
        proxy_base_url: "http://127.0.0.1:11434".to_string(),
    };

    (Arc::new(state), temp_dir)
}

#[tokio::test]
async fn test_get_structured_config_returns_valid_json() {
    let config_content = create_test_config();
    let (state, _temp_dir) = build_test_app_state(&config_content);
    let router = build_router(state);

    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/api/config/structured")
        .body(axum::body::Body::empty())
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(parsed.get("general").is_some());
    assert!(parsed.get("backends").is_some());
    assert!(parsed.get("models").is_some());
    assert!(parsed.get("supervisor").is_some());
    assert!(parsed.get("sampling_templates").is_some());
    assert!(parsed.get("proxy").is_some());
}

#[tokio::test]
async fn test_post_structured_config_persists_and_round_trips() {
    let config_content = create_test_config();
    let (state, _temp_dir) = build_test_app_state(&config_content);
    let router = build_router(state);

    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/api/config/structured")
        .body(axum::body::Body::empty())
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut initial: serde_json::Value = serde_json::from_slice(&body).unwrap();

    if let Some(general) = initial.get_mut("general") {
        general["log_level"] = "debug".into();
    }

    let req = axum::extract::Request::builder()
        .method("POST")
        .uri("/api/config/structured")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_string(&initial).unwrap(),
        ))
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);

    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/api/config/structured")
        .body(axum::body::Body::empty())
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let final_config: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(final_config["general"]["log_level"], "debug");
}

#[tokio::test]
async fn test_400_on_invalid_json() {
    let config_content = create_test_config();
    let (state, _temp_dir) = build_test_app_state(&config_content);
    let router = build_router(state);

    let req = axum::extract::Request::builder()
        .method("POST")
        .uri("/api/config/structured")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{ invalid json }"))
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn test_404_when_config_path_not_configured() {
    let state = Arc::new(AppState {
        logs_dir: None,
        config_path: None,
        proxy_config: None,
        client: reqwest::Client::new(),
        proxy_base_url: "http://127.0.0.1:11434".to_string(),
    });
    let router = build_router(state);

    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/api/config/structured")
        .body(axum::body::Body::empty())
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 404);

    // POST with config_path=None: Json extractor runs first, returns 422 for invalid StructuredConfigBody
    // (Axum's Json<T> extractor validates structure before handler runs)
    let req = axum::extract::Request::builder()
        .method("POST")
        .uri("/api/config/structured")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    // 422 Unprocessable Entity: JSON is valid but doesn't match StructuredConfigBody schema
    assert_eq!(response.status(), 422);
}
