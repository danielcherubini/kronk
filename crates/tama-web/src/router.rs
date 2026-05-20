use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use include_dir::{include_dir, Dir};
use std::sync::Arc;
use tower_http::{catch_panic::CatchPanicLayer, cors::CorsLayer};

use crate::api;
use crate::api::backends::{
    activate_backend_version, check_backend_updates, get_job, install_backend, job_events_sse,
    list_backend_versions, list_backends, remove_backend, remove_backend_version,
    system_capabilities, update_backend, update_backend_default_args,
};
use crate::api::backup::{restore_preview, start_restore};
use crate::api::benchmarks::{
    benchmark_events, delete_benchmark, get_benchmark_result, list_benchmark_history,
    run_benchmark, run_mtp_benchmark, run_spec_benchmark,
};
use tama_core::proxy::forward::forward_request;
use tama_core::proxy::ProxyState;

/// Embedded dist/ directory for serving the web UI.
static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/dist");

/// Serve a static file from the embedded `dist/` directory.
async fn serve_static(path: Option<Path<String>>) -> Response {
    let file_path = path.map(|p| p.0).unwrap_or_else(|| "index.html".into());
    let file_path = if file_path.is_empty() || file_path == "/" {
        "index.html".to_string()
    } else {
        file_path
    };

    match DIST.get_file(&file_path) {
        Some(f) => {
            let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
            Response::builder()
                .header("Content-Type", mime.as_ref())
                .body(Body::from(f.contents()))
                .unwrap()
        }
        None => {
            // SPA fallback: return index.html for unknown paths
            match DIST.get_file("index.html") {
                Some(f) => Html(std::str::from_utf8(f.contents()).unwrap_or("")).into_response(),
                None => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
            }
        }
    }
}

/// Dedicated handler for the root path — avoids Axum type-inference issues with inline closures.
async fn serve_index() -> Response {
    serve_static(None).await
}

/// Serve a static file from dist if it exists, otherwise forward to an available
/// backend. This handles root-level paths like /slots, /tokenize, etc. that the
/// llama.cpp server exposes, while still allowing the web UI's JS/CSS/WASM files
/// to be served from /.
async fn handle_static_or_forward(
    State(state): State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let path = req.uri().path().to_string();
    let file_path = if path.is_empty() || path == "/" {
        "index.html".to_string()
    } else {
        path.trim_start_matches('/').to_string()
    };

    // Try static file first
    if let Some(f) = DIST.get_file(&file_path) {
        let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
        return Response::builder()
            .header("Content-Type", mime.as_ref())
            .body(Body::from(f.contents()))
            .unwrap();
    }

    // File not in dist — forward to an available backend
    let server_name = {
        let models = state.models.read().await;
        models.keys().next().cloned().unwrap_or_default()
    };

    if server_name.is_empty() {
        return (StatusCode::SERVICE_UNAVAILABLE, "No backend server available").into_response();
    }

    let (parts, body) = req.into_parts();
    let body_bytes = axum::body::to_bytes(body, 16 * 1024 * 1024).await.unwrap_or_default();
    forward_request(&state, &server_name, &parts, &body_bytes, None).await
}

/// Build the web UI routes without attaching state.
///
/// The caller (e.g., the proxy server) must call `.with_state(state)` on the
/// returned router before serving. This allows the proxy to merge web routes
/// with its own routes under a single `ProxyState`.
pub fn build_web_routes() -> Router<Arc<tama_core::proxy::ProxyState>> {
    // Build sub-router for backends API with CORS and origin enforcement.
    // CorsLayer must be outermost (applied last) so it runs before same-origin check.
    let backend_routes = Router::new()
        .route("/tama/v1/system/capabilities", get(system_capabilities))
        .route("/tama/v1/backends", get(list_backends))
        // Install/update endpoints: 16MB body limit
        .route(
            "/tama/v1/backends/install",
            post(install_backend).layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/tama/v1/backends/:name/update",
            post(update_backend).layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/tama/v1/backends/:name", delete(remove_backend))
        .route(
            "/tama/v1/backends/:name/default-args",
            post(update_backend_default_args),
        )
        .route(
            "/tama/v1/backends/:name/versions/:version",
            delete(remove_backend_version),
        )
        .route(
            "/tama/v1/backends/check-updates",
            post(check_backend_updates),
        )
        .route(
            "/tama/v1/backends/:name/versions",
            get(list_backend_versions),
        )
        .route(
            "/tama/v1/backends/:name/activate",
            post(activate_backend_version),
        )
        .route("/tama/v1/backends/jobs/:id", get(get_job))
        .route("/tama/v1/backends/jobs/:id/events", get(job_events_sse))
        // Restore routes (CSRF-protected)
        .route("/tama/v1/restore/preview", post(restore_preview))
        .route("/tama/v1/restore", post(start_restore))
        // Self-update POST is inside backend_routes for CSRF protection
        .route(
            "/tama/v1/self-update/update",
            post(api::self_update::trigger_update),
        )
        .route("/tama/v1/updates/check", post(api::updates::trigger_check))
        .route(
            "/tama/v1/updates/check/:item_type/:item_id",
            post(api::updates::check_single),
        )
        .route(
            "/tama/v1/updates/apply/backend/:name",
            post(api::updates::apply_backend_update),
        )
        .route(
            "/tama/v1/updates/apply/model/:id",
            post(api::updates::apply_model_update),
        )
        .route("/tama/v1/updates", get(api::updates::get_updates))
        // CORS layer outermost (applied last) so it runs before same-origin enforcement
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::DELETE,
                ])
                .allow_headers(tower_http::cors::Any)
                // Expose X-CSRF-Token so JS can read it from GET responses
                .expose_headers([axum::http::HeaderName::from_static("x-csrf-token")]),
        )
        .layer(middleware::from_fn(api::middleware::enforce_same_origin));

    // 1MB body limit for all JSON API endpoints
    let json_body_limit = axum::extract::DefaultBodyLimit::max(1024 * 1024);

    // Sub-router for non-backend state-changing endpoints with CSRF enforcement
    let csrf_routes = Router::new()
        .route(
            "/tama/v1/config",
            get(api::get_config)
                .post(api::save_config)
                .layer(json_body_limit),
        )
        .route(
            "/tama/v1/config/structured",
            get(api::get_structured_config)
                .post(api::save_structured_config)
                .layer(json_body_limit),
        )
        .route(
            "/tama/v1/models",
            get(api::list_models)
                .post(api::create_model)
                .layer(json_body_limit),
        )
        .route(
            "/tama/v1/models/:id",
            get(api::get_model)
                .put(api::update_model)
                .delete(api::delete_model),
        )
        .route(
            "/tama/v1/models/:id/rename",
            post(api::rename_model).layer(json_body_limit),
        )
        .route(
            "/tama/v1/models/:id/refresh",
            post(api::refresh_model_metadata).layer(json_body_limit),
        )
        .route(
            "/tama/v1/models/:id/verify",
            post(api::verify_model_files).layer(json_body_limit),
        )
        .route(
            "/tama/v1/models/:id/quants/:quant_key",
            delete(api::delete_quant),
        )
        .route(
            "/tama/v1/benchmarks/run",
            post(run_benchmark).layer(json_body_limit),
        )
        .route(
            "/tama/v1/benchmarks/spec-run",
            post(run_spec_benchmark).layer(json_body_limit),
        )
        .route(
            "/tama/v1/benchmarks/mtp-run",
            post(run_mtp_benchmark).layer(json_body_limit),
        )
        .route(
            "/tama/v1/downloads/:job_id/cancel",
            post(api::downloads::cancel_download).layer(json_body_limit),
        )
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                ])
                .allow_headers(tower_http::cors::Any)
                // Expose X-CSRF-Token so JS can read it from GET responses
                .expose_headers([axum::http::HeaderName::from_static("x-csrf-token")]),
        )
        .layer(middleware::from_fn(api::middleware::enforce_same_origin));

    Router::new()
        // HF metadata endpoint — wildcard captures `owner/repo` with embedded slash
        .route("/tama/v1/hf/*repo_id", get(api::hf::hf_metadata))
        // Self-update GET routes (safe methods, no CSRF protection needed)
        .route(
            "/tama/v1/self-update/check",
            get(api::self_update::check_update),
        )
        .route(
            "/tama/v1/self-update/events",
            get(api::self_update::update_events),
        )
        // Benchmark GET routes (no CSRF needed)
        .route("/tama/v1/benchmarks/jobs/:id", get(get_benchmark_result))
        .route("/tama/v1/benchmarks/jobs/:id/events", get(benchmark_events))
        .route("/tama/v1/benchmarks/history", get(list_benchmark_history))
        .route("/tama/v1/benchmarks/history/:id", delete(delete_benchmark))
        // Downloads Center routes
        .route(
            "/tama/v1/downloads/active",
            get(api::downloads::get_active_downloads),
        )
        .route(
            "/tama/v1/downloads/history",
            get(api::downloads::get_download_history),
        )
        .route(
            "/tama/v1/downloads/events",
            get(api::downloads::download_events_sse),
        )
        // API documentation (OpenAPI 3.1.0 spec)
        .route("/tama/v1/docs", get(api::openapi::serve_spec))
        // Logs: backend-specific log retrieval
        .route("/tama/v1/logs/:backend", get(api::logs::get_backend_logs))
        .merge(csrf_routes)
        .merge(backend_routes)
        // Web UI — mounted at /ui
        .route("/ui", get(serve_index))
        .route(
            "/ui/*path",
            get(|Path(p): Path<String>| async move { serve_static(Some(Path(p))).await }),
        )
        // Root-level static files + backend forwarding for unknown paths
        .route("/*path", get(handle_static_or_forward))
        .layer(CatchPanicLayer::new())
}
