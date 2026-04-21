//! Integration tests for the Downloads Center API endpoints.

use axum::{body::Body, http::Request, Router};
use std::sync::Arc;
use tower::ServiceExt;

use koji_web::api::downloads::{
    DownloadCancelResponse, DownloadsActiveResponse, DownloadsHistoryResponse,
};
use koji_web::server::AppState;

/// Create a test AppState with an in-memory download queue service.
fn create_test_state() -> Arc<AppState> {
    use koji_core::proxy::download_queue::DownloadQueueService;

    let tmp = tempfile::tempdir().unwrap();
    let db_dir = tmp.path().to_path_buf();

    // Initialize the database
    let svc = DownloadQueueService::new(Some(db_dir), 2);
    let _ = svc.open_conn().unwrap();

    Arc::new(AppState {
        proxy_base_url: "http://localhost:8080".to_string(),
        client: reqwest::Client::new(),
        logs_dir: None,
        config_path: None,
        proxy_config: None,
        jobs: None,
        capabilities: None,
        update_checker: Arc::new(koji_core::updates::UpdateChecker::new()),
        binary_version: "0.0.0-test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        download_queue: Some(Arc::new(svc)),
    })
}

/// Build the router with the given state, including only downloads routes.
fn build_download_router(state: Arc<AppState>) -> Router {
    use axum::routing::{get, post};

    Router::new()
        .route(
            "/koji/v1/downloads/active",
            get(koji_web::api::downloads::get_active_downloads),
        )
        .route(
            "/koji/v1/downloads/history",
            get(koji_web::api::downloads::get_download_history),
        )
        .route(
            "/koji/v1/downloads/:job_id/cancel",
            post(koji_web::api::downloads::cancel_download),
        )
        .with_state(state)
}

/// Seed the download queue with test data:
/// - 2 active items (queued + running)
/// - 3 history items (completed, failed, cancelled)
fn seed_test_data(state: &AppState) {
    let svc = state
        .download_queue
        .as_ref()
        .expect("download_queue configured");

    // Active items
    svc.enqueue(
        "job-active-1",
        "unsloth/Qwen3.6-35B-A3B-GGUF",
        "Qwen3.6-35B-Q4_K_M.gguf",
        Some("Qwen3.6 35B"),
        "model",
        Some("Q4_K_M"),
        Some(4096),
    )
    .unwrap();

    svc.enqueue(
        "job-active-2",
        "meta-llama/Llama-3.1-8B-GGUF",
        "Llama-3.1-8B-Q5_K_M.gguf",
        Some("Llama 3.1 8B"),
        "model",
        Some("Q5_K_M"),
        Some(8192),
    )
    .unwrap();

    // Transition job-active-2 to running with some progress
    svc.update_status("job-active-2", "running", 1500, Some(3000), None, None)
        .unwrap();

    // History items
    svc.enqueue(
        "job-history-1",
        "test/repo",
        "model1.gguf",
        None,
        "model",
        None,
        None,
    )
    .unwrap();
    svc.update_status(
        "job-history-1",
        "completed",
        2000,
        Some(2000),
        None,
        Some(5000),
    )
    .unwrap();

    svc.enqueue(
        "job-history-2",
        "test/repo",
        "model2.gguf",
        None,
        "model",
        None,
        None,
    )
    .unwrap();
    svc.update_status(
        "job-history-2",
        "failed",
        500,
        Some(2000),
        Some("LFS hash mismatch"),
        None,
    )
    .unwrap();

    svc.enqueue(
        "job-history-3",
        "test/repo",
        "model3.gguf",
        None,
        "model",
        None,
        None,
    )
    .unwrap();
    svc.update_status("job-history-3", "cancelled", 100, Some(2000), None, None)
        .unwrap();
}

#[tokio::test]
async fn test_get_active_downloads_returns_correct_dtos() {
    let state = create_test_state();
    seed_test_data(&state);

    let app = build_download_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/koji/v1/downloads/active")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let response_obj: DownloadsActiveResponse =
        serde_json::from_value(json).expect("valid DownloadsActiveResponse");

    // Should have 2 active items (queued + running)
    assert_eq!(response_obj.items.len(), 2);

    let queued_item = response_obj
        .items
        .iter()
        .find(|i| i.job_id == "job-active-1")
        .expect("should find queued item");
    assert_eq!(queued_item.status, "queued");
    // progress_percent is computed client-side from bytes_downloaded/total_bytes

    let running_item = response_obj
        .items
        .iter()
        .find(|i| i.job_id == "job-active-2")
        .expect("should find running item");
    assert_eq!(running_item.status, "running");
    // progress_percent is computed client-side from bytes_downloaded/total_bytes

    // Verify DTO fields are populated
    assert_eq!(queued_item.repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
    assert_eq!(queued_item.filename, "Qwen3.6-35B-Q4_K_M.gguf");
    assert_eq!(queued_item.display_name, Some("Qwen3.6 35B".to_string()));
    assert_eq!(queued_item.kind, "model");
}

#[tokio::test]
async fn test_get_download_history_with_pagination() {
    let state = create_test_state();
    seed_test_data(&state);

    // Test default pagination (limit=50, offset=0)
    let app = build_download_router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/koji/v1/downloads/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let response_obj: DownloadsHistoryResponse =
        serde_json::from_value(json).expect("valid DownloadsHistoryResponse");

    assert_eq!(response_obj.total, 3);
    assert_eq!(response_obj.items.len(), 3);

    // Test with limit=1
    let app = build_download_router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/koji/v1/downloads/history?limit=1&offset=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let response_obj: DownloadsHistoryResponse =
        serde_json::from_value(json).expect("valid DownloadsHistoryResponse");

    assert_eq!(response_obj.total, 3);
    assert_eq!(response_obj.items.len(), 1);

    // Test with offset=1, limit=1
    let app = build_download_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/koji/v1/downloads/history?limit=1&offset=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let response_obj: DownloadsHistoryResponse =
        serde_json::from_value(json).expect("valid DownloadsHistoryResponse");

    assert_eq!(response_obj.total, 3);
    assert_eq!(response_obj.items.len(), 1);
}

#[tokio::test]
async fn test_cancel_download_succeeds_for_queued_item() {
    let state = create_test_state();
    seed_test_data(&state);

    let svc = state.download_queue.as_ref().unwrap();
    let _ = svc.open_conn().unwrap();

    let app = build_download_router(state);

    // Cancel the queued item (job-active-1)
    let response = app
        .oneshot(
            Request::builder()
                .uri("/koji/v1/downloads/job-active-1/cancel")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let response_obj: DownloadCancelResponse =
        serde_json::from_value(json).expect("valid DownloadCancelResponse");

    assert!(response_obj.ok);
    assert!(response_obj.message.is_none());
}

#[tokio::test]
async fn test_cancel_download_returns_error_for_completed_item() {
    let state = create_test_state();
    seed_test_data(&state);

    let app = build_download_router(state);

    // Try to cancel an already completed item (job-history-1)
    let response = app
        .oneshot(
            Request::builder()
                .uri("/koji/v1/downloads/job-history-1/cancel")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let response_obj: DownloadCancelResponse =
        serde_json::from_value(json).expect("valid DownloadCancelResponse");

    assert!(!response_obj.ok);
    assert!(response_obj.message.is_some());
    let msg = response_obj.message.unwrap();
    assert!(msg.contains("terminal state"));
}

#[tokio::test]
async fn test_cancel_nonexistent_item_returns_error() {
    let state = create_test_state();
    seed_test_data(&state);

    let app = build_download_router(state);

    // Try to cancel a non-existent item
    let response = app
        .oneshot(
            Request::builder()
                .uri("/koji/v1/downloads/nonexistent-job/cancel")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let response_obj: DownloadCancelResponse =
        serde_json::from_value(json).expect("valid DownloadCancelResponse");

    assert!(!response_obj.ok);
    assert!(response_obj.message.is_some());
}
