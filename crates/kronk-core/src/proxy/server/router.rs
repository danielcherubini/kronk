use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use crate::proxy::handlers::{
    handle_chat_completions, handle_fallback, handle_get_model, handle_health, handle_list_models,
    handle_metrics, handle_status, handle_stream_chat_completions,
};
use crate::proxy::kronk_handlers::{
    handle_hf_list_quants, handle_kronk_get_model as handle_kronk_get_model_fn,
    handle_kronk_get_pull_job, handle_kronk_list_models, handle_kronk_load_model,
    handle_kronk_pull_model, handle_kronk_system_health, handle_kronk_system_restart,
    handle_kronk_unload_model,
};
use crate::proxy::ProxyState;

/// Build the axum router with all proxy routes and shared state.
pub fn build_router(state: Arc<ProxyState>) -> Router {
    Router::new()
        // OpenAI-compatible routes
        // Some clients (e.g. those with base_url = http://host/v1) POST directly to /v1
        .route("/v1", post(handle_chat_completions))
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
        // Kronk management API — model lifecycle
        .route("/kronk/v1/models", get(handle_kronk_list_models))
        .route("/kronk/v1/models/:id", get(handle_kronk_get_model_fn))
        .route("/kronk/v1/models/:id/load", post(handle_kronk_load_model))
        .route(
            "/kronk/v1/models/:id/unload",
            post(handle_kronk_unload_model),
        )
        // Pull jobs live under /kronk/v1/pulls/ to avoid path conflict with /models/:id
        .route("/kronk/v1/pulls", post(handle_kronk_pull_model))
        .route("/kronk/v1/pulls/:job_id", get(handle_kronk_get_pull_job))
        // HuggingFace quant listing — wildcard captures `owner/repo` with embedded slash
        .route("/kronk/v1/hf/*repo_id", get(handle_hf_list_quants))
        // System
        .route("/kronk/v1/system/health", get(handle_kronk_system_health))
        .route(
            "/kronk/v1/system/restart",
            post(handle_kronk_system_restart),
        )
        .fallback(handle_fallback)
        .with_state(state)
        .layer(CorsLayer::permissive())
}
