use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::server::AppState;

/// GET /tama/v1/hf/:repo_id/metadata — fetch HuggingFace model metadata (API + README).
/// Returns `HfModelMetadata` with fields populated from HF API tags and README parsing.
pub async fn hf_metadata(
    State(_state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> impl IntoResponse {
    match tama_core::models::pull::fetch_hf_metadata(&repo_id).await {
        Ok(meta) => (StatusCode::OK, Json(meta)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
