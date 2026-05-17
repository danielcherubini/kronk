use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::server::AppState;

/// GET /tama/v1/hf/*repo_id — fetch HuggingFace model metadata (API + README).
/// Wildcard captures `owner/repo/metadata`; we strip the trailing `/metadata`.
/// Returns `HfModelMetadata` with fields populated from HF API tags and README parsing.
pub async fn hf_metadata(
    State(_state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> impl IntoResponse {
    // Strip trailing "/metadata" from the wildcard path
    let repo_id = match path.strip_suffix("/metadata") {
        Some(r) => r,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Path must end with /metadata" })),
            )
                .into_response();
        }
    };

    match tama_core::models::pull::fetch_hf_metadata(repo_id).await {
        Ok(meta) => (StatusCode::OK, Json(meta)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
