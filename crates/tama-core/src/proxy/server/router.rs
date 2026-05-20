use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
#[cfg(feature = "web-ui")]
use tower_http::catch_panic::CatchPanicLayer;
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
    handle_opencode_list_models, handle_pull_job_stream, handle_system_metrics_stream,
    handle_tama_get_model as handle_tama_get_model_fn, handle_tama_get_pull_job,
    handle_tama_list_models, handle_tama_load_model, handle_tama_pull_model,
    handle_tama_system_health, handle_tama_system_restart, handle_tama_unload_model,
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

/// Build a unified axum Router that merges proxy routes with an extra router
/// (e.g., web UI routes from `tama-web`).
///
/// Route priority is critical: proxy-specific routes (e.g., `/tama/v1/models/:id/load`)
/// must be defined before extra catch-alls (e.g., `/tama/v1/models/:id`) so that
/// axum matches the more specific handler first.
///
/// Routes that overlap with the web UI (`/tama/v1/models`, `/tama/v1/models/:id`,
/// `/tama/v1/system/health`, `/tama/v1/logs/:backend`) are **not** defined in the
/// proxy sub-router — the web UI handles those. Only proxy-exclusive routes and
/// specific sub-paths (e.g., `:id/load`, `:id/unload`) are included here.
///
/// The `extra_routes` parameter is a `Router` already typed with `Arc<ProxyState>`
/// but without `.with_state()` called. This function merges proxy routes first
/// (higher priority), then extra routes, and applies shared layers + state.
#[cfg(feature = "web-ui")]
pub fn build_unified_router(
    state: Arc<ProxyState>,
    extra_routes: Router<Arc<ProxyState>>,
) -> Router {
    // Build proxy routes — these take priority over extra (web UI) routes.
    // NOTE: Routes that overlap with web UI are intentionally excluded:
    // - /tama/v1/models (GET) — web UI handles CRUD
    // - /tama/v1/models/:id (GET) — web UI handles CRUD
    // - /tama/v1/system/health (GET) — web UI re-exports proxy handler
    // - /tama/v1/logs/:backend (GET) — web UI has its own handler
    // - /tama/v1/hf/*repo_id (GET) — web UI handles HF metadata
    let proxy_routes = Router::new()
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
        // Tama management API — model lifecycle (specific routes before web catch-alls)
        .route("/tama/v1/models/:id/load", post(handle_tama_load_model))
        .route("/tama/v1/models/:id/unload", post(handle_tama_unload_model))
        // OpenCode plugin discovery API — returns rich model metadata
        .route("/tama/v1/opencode/models", get(handle_opencode_list_models))
        // Pull jobs live under /tama/v1/pulls/ to avoid path conflict with /models/:id
        .route("/tama/v1/pulls", post(handle_tama_pull_model))
        .route("/tama/v1/pulls/:job_id", get(handle_tama_get_pull_job))
        .route("/tama/v1/pulls/:job_id/stream", get(handle_pull_job_stream))
        // NOTE: /tama/v1/hf/*repo_id excluded — web UI handles HF metadata
        // System
        .route(
            "/tama/v1/system/reload-configs",
            post(handle_reload_configs),
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
        // Wildcard POST forwarding for backend endpoints (completions, tokenize, slots, etc.)
        .route("/*path", post(handle_forward_post));

    // Proxy routes first (higher priority), then extra routes.
    // NOTE: Wildcard forwarding routes (*path) and fallback are NOT included
    // here because the web UI (extra_routes) provides its own static file
    // serving and SPA fallback. Including them would conflict.
    Router::new()
        .merge(proxy_routes)
        .merge(extra_routes)
        .layer(CorsLayer::permissive())
        .layer(CatchPanicLayer::new())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that the proxy router returns 200 for known proxy endpoints.
    #[tokio::test]
    async fn test_proxy_router_serves_known_routes() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config, None));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let app = build_router(state.clone());
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();

        // Health endpoint
        let resp = client
            .get(format!("http://{}/health", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Models endpoint
        let resp = client
            .get(format!("http://{}/v1/models", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Tama system health
        let resp = client
            .get(format!("http://{}/tama/v1/system/health", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    /// Verify that proxy-specific routes take priority over extra catch-alls.
    /// The `/tama/v1/models/:id/load` endpoint should return a proxy response,
    /// not a 405 from the extra router, proving the route ordering is correct.
    #[cfg(feature = "web-ui")]
    #[tokio::test]
    async fn test_unified_router_route_priority() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config, None));

        // Simulate web UI routes: PUT/DELETE on /tama/v1/models/:id
        // (GET is handled by web UI in the real app, not defined here to avoid overlap)
        let extra_routes = Router::<Arc<crate::proxy::ProxyState>>::new().route(
            "/tama/v1/models/:id",
            axum::routing::put(|| async { "web put " }).delete(|| async { "web delete " }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let app = build_unified_router(state.clone(), extra_routes);
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();

        // POST to /tama/v1/models/test/load — should be handled by proxy's
        // handle_tama_load_model, not by extra router's catch-all.
        let resp = client
            .post(format!("http://{}/tama/v1/models/test/load", bound_addr))
            .send()
            .await
            .unwrap();
        // Must NOT be 405 (Method Not Allowed) — that would mean the extra
        // route for /tama/v1/models/:id matched instead of our proxy route.
        assert_ne!(
            resp.status(),
            405,
            "Route priority failed: extra router caught /tama/v1/models/:id/load instead of proxy handler"
        );

        // POST to /tama/v1/models/test/unload — same priority check
        let resp = client
            .post(format!("http://{}/tama/v1/models/test/unload", bound_addr))
            .send()
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            405,
            "Route priority failed: extra router caught /tama/v1/models/:id/unload instead of proxy handler"
        );

        // GET /health — proxy route
        let resp = client
            .get(format!("http://{}/health", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // GET /v1/models — proxy route
        let resp = client
            .get(format!("http://{}/v1/models", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}
