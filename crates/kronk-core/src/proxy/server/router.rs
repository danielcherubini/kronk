use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::proxy::handlers::{
    handle_chat_completions, handle_fallback, handle_get_model, handle_health, handle_list_models,
    handle_metrics, handle_status, handle_stream_chat_completions,
};
use crate::proxy::ProxyState;

/// Build the axum router with all proxy routes and shared state.
pub fn build_router(state: Arc<ProxyState>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route(
            "/v1/chat/completions/stream",
            post(handle_stream_chat_completions),
        )
        .route("/v1/models", get(handle_list_models))
        .route("/v1/models/:model_id", get(handle_get_model))
        .route("/status", get(handle_status))
        .route("/health", get(handle_health))
        .route("/metrics", get(handle_metrics))
        .fallback(handle_fallback)
        .with_state(state)
}
