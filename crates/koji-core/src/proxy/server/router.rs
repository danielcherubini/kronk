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
use crate::proxy::koji_handlers::{
    handle_hf_list_quants, handle_koji_get_model as handle_koji_get_model_fn,
    handle_koji_get_pull_job, handle_koji_list_models, handle_koji_load_model,
    handle_koji_pull_model, handle_koji_system_health, handle_koji_system_restart,
    handle_koji_unload_model, handle_opencode_list_models, handle_pull_job_stream,
    handle_system_metrics_history, handle_system_metrics_stream,
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
        // Koji management API — model lifecycle
        .route("/koji/v1/models", get(handle_koji_list_models))
        .route("/koji/v1/models/:id", get(handle_koji_get_model_fn))
        .route("/koji/v1/models/:id/load", post(handle_koji_load_model))
        .route("/koji/v1/models/:id/unload", post(handle_koji_unload_model))
        // OpenCode plugin discovery API — returns rich model metadata
        .route("/koji/v1/opencode/models", get(handle_opencode_list_models))
        // Pull jobs live under /koji/v1/pulls/ to avoid path conflict with /models/:id
        .route("/koji/v1/pulls", post(handle_koji_pull_model))
        .route("/koji/v1/pulls/:job_id", get(handle_koji_get_pull_job))
        .route("/koji/v1/pulls/:job_id/stream", get(handle_pull_job_stream))
        // HuggingFace quant listing — wildcard captures `owner/repo` with embedded slash
        .route("/koji/v1/hf/*repo_id", get(handle_hf_list_quants))
        // System
        .route("/koji/v1/system/health", get(handle_koji_system_health))
        .route(
            "/koji/v1/system/metrics/history",
            get(handle_system_metrics_history),
        )
        .route(
            "/koji/v1/system/metrics/stream",
            get(handle_system_metrics_stream),
        )
        .route("/koji/v1/system/restart", post(handle_koji_system_restart))
        .fallback(handle_fallback)
        .with_state(state)
        .layer(CorsLayer::permissive())
}
