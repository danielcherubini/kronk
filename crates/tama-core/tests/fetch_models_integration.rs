use tama_core::config::Config;
use tama_core::proxy::{fetch_models_from_backend, parse_models_response, ProxyState};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Test parse_models_response with valid data
#[test]
fn test_parse_models_response_integration_valid() {
    let body = r#"{
        "object": "list",
        "data": [
            {"id": "model-a", "object": "model"},
            {"id": "model-b", "object": "model"}
        ]
    }"#;
    let result = parse_models_response(body.as_bytes());
    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["id"], "model-a");
    assert_eq!(result[1]["id"], "model-b");
}

/// Test parse_models_response with invalid JSON
#[test]
fn test_parse_models_response_integration_invalid_json() {
    let result = parse_models_response(b"not json at all {{{");
    assert!(result.is_empty());
}

/// Test parse_models_response with missing data field
#[test]
fn test_parse_models_response_integration_missing_data() {
    let body = r#"{"object": "list"}"#;
    let result = parse_models_response(body.as_bytes());
    assert!(result.is_empty());
}

/// Test parse_models_response with data as non-array
#[test]
fn test_parse_models_response_integration_data_not_array() {
    let body = r#"{"data": {"id": "single"}}"#;
    let result = parse_models_response(body.as_bytes());
    assert!(result.is_empty());
}

/// Integration test: fetch_models_from_backend returns data from mock server
#[tokio::test]
async fn test_fetch_models_from_backend_returns_data() {
    let mock_server = MockServer::start().await;

    let response_body = serde_json::json!({
        "object": "list",
        "data": [
            {"id": "mock-model-1", "object": "model"},
            {"id": "mock-model-2", "object": "model"}
        ]
    });

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
        .mount(&mock_server)
        .await;

    let config = Config::default();
    let state = ProxyState::new(config, None);

    let result = fetch_models_from_backend(&state, &mock_server.uri()).await;

    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["id"], "mock-model-1");
    assert_eq!(result[1]["id"], "mock-model-2");
}

/// Integration test: fetch_models_from_backend returns empty Vec on invalid response
#[tokio::test]
async fn test_fetch_models_from_backend_invalid_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&mock_server)
        .await;

    let config = Config::default();
    let state = ProxyState::new(config, None);

    let result = fetch_models_from_backend(&state, &mock_server.uri()).await;

    assert!(result.is_empty());
}

/// Integration test: fetch_models_from_backend returns empty Vec on 500 error
#[tokio::test]
async fn test_fetch_models_from_backend_server_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    let config = Config::default();
    let state = ProxyState::new(config, None);

    let result = fetch_models_from_backend(&state, &mock_server.uri()).await;

    assert!(result.is_empty());
}

/// Integration test: fetch_models_from_backend returns empty Vec on connection refused
#[tokio::test]
async fn test_fetch_models_from_backend_connection_refused() {
    let config = Config::default();
    let state = ProxyState::new(config, None);

    // Use a port that nothing is listening on
    let result = fetch_models_from_backend(&state, "http://localhost:59999").await;

    assert!(result.is_empty());
}
