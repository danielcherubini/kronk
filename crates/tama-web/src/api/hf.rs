use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use tama_core::proxy::ProxyState;

/// GET /tama/v1/hf/*repo_id — fetch HuggingFace model metadata (API + README).
/// Wildcard captures `owner/repo/metadata`; we strip the trailing `/metadata`.
/// If path doesn't end with `/metadata`, forward to proxy for quant list handling.
pub async fn hf_metadata(
    State(state): State<Arc<ProxyState>>,
    Path(path): Path<String>,
) -> axum::http::Response<axum::body::Body> {
    // Strip trailing "/metadata" from the wildcard path
    let repo_id = match path.strip_suffix("/metadata") {
        Some(r) => r,
        None => {
            // Not a metadata request — forward to proxy for quant list handling
            return proxy_hf_request(&state, &path).await;
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

/// Forward a non-metadata HF request to the proxy server.
async fn proxy_hf_request(
    state: &Arc<ProxyState>,
    path: &str,
) -> axum::http::Response<axum::body::Body> {
    let url = format!(
        "{}/tama/v1/hf/{}",
        state.config.read().await.proxy_url(),
        path
    );
    match reqwest::get(&url).await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.bytes().await.unwrap_or_default();
            (status, body).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Failed to reach proxy: {}", e) })),
        )
            .into_response(),
    }
}
