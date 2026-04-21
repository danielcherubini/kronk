use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use crate::server::AppState;

/// GET /koji/v1/system/capabilities
pub async fn system_capabilities(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cache = match &state.capabilities {
        Some(c) => c,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "capabilities cache not configured"})),
            )
                .into_response();
        }
    };

    match cache
        .get_or_compute(
            koji_core::gpu::detect_build_prerequisites,
            koji_core::gpu::detect_cuda_version,
        )
        .await
    {
        Ok(caps) => Json(caps).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
