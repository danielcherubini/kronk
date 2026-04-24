use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use crate::proxy::handlers::tts::{
    handle_audio_models, handle_audio_speech, handle_audio_stream, handle_audio_voices,
};
use crate::proxy::handlers::{
    handle_chat_completions, handle_fallback, handle_forward_get, handle_forward_post,
    handle_get_model, handle_health, handle_list_models, handle_metrics, handle_reload_configs,
    handle_status, handle_stream_chat_completions,
};
use crate::proxy::tama_handlers::{
    backend_logs::handle_all_logs, handle_backend_log_sse, handle_hf_list_quants,
    handle_opencode_list_models, handle_pull_job_stream, handle_system_metrics_history,
    handle_system_metrics_stream, handle_tama_get_model as handle_tama_get_model_fn,
    handle_tama_get_pull_job, handle_tama_list_models, handle_tama_load_model,
    handle_tama_pull_model, handle_tama_system_health, handle_tama_system_restart,
    handle_tama_unload_model,
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
        // Tama management API — model lifecycle
        .route("/tama/v1/models", get(handle_tama_list_models))
        .route("/tama/v1/models/:id", get(handle_tama_get_model_fn))
        .route("/tama/v1/models/:id/load", post(handle_tama_load_model))
        .route("/tama/v1/models/:id/unload", post(handle_tama_unload_model))
        // OpenCode plugin discovery API — returns rich model metadata
        .route("/tama/v1/opencode/models", get(handle_opencode_list_models))
        // Pull jobs live under /tama/v1/pulls/ to avoid path conflict with /models/:id
        .route("/tama/v1/pulls", post(handle_tama_pull_model))
        .route("/tama/v1/pulls/:job_id", get(handle_tama_get_pull_job))
        .route("/tama/v1/pulls/:job_id/stream", get(handle_pull_job_stream))
        // HuggingFace quant listing — wildcard captures `owner/repo` with embedded slash
        .route("/tama/v1/hf/*repo_id", get(handle_hf_list_quants))
        // System
        .route("/tama/v1/system/health", get(handle_tama_system_health))
        .route(
            "/tama/v1/system/reload-configs",
            post(handle_reload_configs),
        )
        .route(
            "/tama/v1/system/metrics/history",
            get(handle_system_metrics_history),
        )
        .route(
            "/tama/v1/system/metrics/stream",
            get(handle_system_metrics_stream),
        )
        .route("/tama/v1/system/restart", post(handle_tama_system_restart))
        // Backend log endpoints
        .route("/tama/v1/logs", get(handle_all_logs))
        .route("/tama/v1/logs/:backend/events", get(handle_backend_log_sse))
        // TTS (Text-to-Speech) endpoints - OpenAI-compatible
        .route("/v1/audio/models", get(handle_audio_models))
        .route("/v1/audio/speech", post(handle_audio_speech))
        .route("/v1/audio/speech/stream", post(handle_audio_stream))
        .route("/v1/audio/voices", get(handle_audio_voices))
        // Wildcard forwarding for all other endpoints (llama.cpp API)
        .route("/*path", post(handle_forward_post))
        .route("/*path", get(handle_forward_get))
        .fallback(handle_fallback)
        .with_state(state)
        .layer(CorsLayer::permissive())
}
