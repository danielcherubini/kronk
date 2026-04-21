use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{any, delete, get, post},
    Router,
};
use include_dir::{include_dir, Dir};
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::{catch_panic::CatchPanicLayer, cors::CorsLayer};

use crate::api;
use crate::api::backends::{
    activate_backend_version, check_backend_updates, get_job, install_backend, job_events_sse,
    list_backend_versions, list_backends, remove_backend, remove_backend_version,
    system_capabilities, update_backend, update_backend_default_args, CapabilitiesCache,
};
use crate::api::backup::{restore_preview, start_restore};
use crate::api::benchmarks::{
    benchmark_events, delete_benchmark, get_benchmark_result, list_benchmark_history, run_benchmark,
};
use crate::jobs::JobManager;
#[allow(unused_imports)]
use koji_core::proxy::download_queue::DownloadQueueService;

static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/dist");

#[derive(Clone)]
pub struct AppState {
    pub proxy_base_url: String,
    pub client: reqwest::Client,
    pub logs_dir: Option<std::path::PathBuf>,
    pub config_path: Option<std::path::PathBuf>,
    pub proxy_config: Option<Arc<tokio::sync::RwLock<koji_core::config::Config>>>,
    pub jobs: Option<Arc<JobManager>>,
    pub capabilities: Option<Arc<CapabilitiesCache>>,
    /// Shared update checker to prevent concurrent runs across requests.
    pub update_checker: Arc<koji_core::updates::UpdateChecker>,
    /// The version of the running koji binary (passed from the CLI at startup).
    pub binary_version: String,
    /// Broadcast sender for self-update progress messages.
    /// `None` when no update is in progress.
    pub update_tx: Arc<tokio::sync::Mutex<Option<broadcast::Sender<String>>>>,
    /// Temporary upload storage for restore archives.
    pub upload_lock:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, api::backup::UploadEntry>>>,
    /// Download queue service for managing download lifecycle and events.
    pub download_queue: Option<Arc<DownloadQueueService>>,
}

impl AppState {
    /// Get the temp uploads directory path.
    pub fn temp_uploads_dir(&self) -> std::path::PathBuf {
        self.config_path
            .as_ref()
            .map(|p| p.parent().unwrap_or(p.as_path()).join("uploads"))
            .unwrap_or_else(|| std::env::temp_dir().join("koji_uploads"))
    }
}

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

/// Forward a request to the Koji proxy at `/koji/v1/<path>`.
/// Only allows GET, POST, and PATCH methods; returns 405 for others.
async fn proxy_koji(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
    path: Path<String>,
    body: Body,
) -> Response {
    // Whitelist allowed methods
    if !matches!(method, Method::GET | Method::POST | Method::PATCH) {
        return (StatusCode::METHOD_NOT_ALLOWED, "Method not allowed").into_response();
    }

    let url = format!("{}/koji/v1/{}", state.proxy_base_url, path.0);
    // Cap at 16 MiB — same as MAX_REQUEST_BODY_SIZE in koji-core — to prevent memory exhaustion.
    let body_bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Failed to read request body: {e}"),
            )
                .into_response();
        }
    };

    let mut req = state.client.request(method, &url);
    for (k, v) in &headers {
        if k != axum::http::header::HOST {
            req = req.header(k, v);
        }
    }
    req = req.body(body_bytes);

    match req.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let resp_headers = resp.headers().clone();

            // For SSE (and any streaming response), stream the body directly rather than
            // buffering it — resp.bytes().await would block until the stream closes, making
            // SSE appear broken from the browser's perspective.
            let is_sse = resp_headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.starts_with("text/event-stream"))
                .unwrap_or(false);

            let body = if is_sse {
                let stream = resp.bytes_stream();
                Body::from_stream(stream)
            } else {
                let bytes = resp.bytes().await.unwrap_or_default();
                Body::from(bytes)
            };

            let mut response = Response::new(body);
            *response.status_mut() = status;
            for (k, v) in &resp_headers {
                response.headers_mut().insert(k, v.clone());
            }
            response
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("Failed to reach Koji proxy: {e}"),
        )
            .into_response(),
    }
}

/// Dedicated handler for the root path — avoids Axum type-inference issues with inline closures.
async fn serve_index() -> Response {
    serve_static(None).await
}

pub fn build_router(state: Arc<AppState>) -> Router {
    // Build sub-router for backends API with CORS and origin enforcement.
    // CorsLayer must be outermost (applied last) so it runs before same-origin check.
    let backend_routes = Router::new()
        .route("/koji/v1/system/capabilities", get(system_capabilities))
        .route("/koji/v1/backends", get(list_backends))
        // Install/update endpoints: 16MB body limit
        .route(
            "/koji/v1/backends/install",
            post(install_backend).layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/koji/v1/backends/:name/update",
            post(update_backend).layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/koji/v1/backends/:name", delete(remove_backend))
        .route(
            "/koji/v1/backends/:name/default-args",
            post(update_backend_default_args),
        )
        .route(
            "/koji/v1/backends/:name/versions/:version",
            delete(remove_backend_version),
        )
        .route(
            "/koji/v1/backends/check-updates",
            post(check_backend_updates),
        )
        .route(
            "/koji/v1/backends/:name/versions",
            get(list_backend_versions),
        )
        .route(
            "/koji/v1/backends/:name/activate",
            post(activate_backend_version),
        )
        .route("/koji/v1/backends/jobs/:id", get(get_job))
        .route("/koji/v1/backends/jobs/:id/events", get(job_events_sse))
        // Restore routes (CSRF-protected)
        .route("/koji/v1/restore/preview", post(restore_preview))
        .route("/koji/v1/restore", post(start_restore))
        // Self-update POST is inside backend_routes for CSRF protection
        .route(
            "/koji/v1/self-update/update",
            post(api::self_update::trigger_update),
        )
        .route("/koji/v1/updates/check", post(api::updates::trigger_check))
        .route(
            "/koji/v1/updates/check/:item_type/:item_id",
            post(api::updates::check_single),
        )
        .route(
            "/koji/v1/updates/apply/backend/:name",
            post(api::updates::apply_backend_update),
        )
        .route(
            "/koji/v1/updates/apply/model/:id",
            post(api::updates::apply_model_update),
        )
        .route("/koji/v1/updates", get(api::updates::get_updates))
        // CORS layer outermost (applied last) so it runs before same-origin enforcement
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::DELETE,
                ])
                .allow_headers(tower_http::cors::Any),
        )
        .layer(middleware::from_fn(api::middleware::enforce_same_origin));

    // 1MB body limit for all JSON API endpoints
    let json_body_limit = axum::extract::DefaultBodyLimit::max(1024 * 1024);

    // Sub-router for non-backend state-changing endpoints with CSRF enforcement
    let csrf_routes = Router::new()
        .route(
            "/koji/v1/config",
            get(api::get_config)
                .post(api::save_config)
                .layer(json_body_limit),
        )
        .route(
            "/koji/v1/config/structured",
            get(api::get_structured_config)
                .post(api::save_structured_config)
                .layer(json_body_limit),
        )
        .route(
            "/koji/v1/models",
            get(api::list_models)
                .post(api::create_model)
                .layer(json_body_limit),
        )
        .route(
            "/koji/v1/models/:id",
            get(api::get_model)
                .put(api::update_model)
                .delete(api::delete_model),
        )
        .route(
            "/koji/v1/models/:id/rename",
            post(api::rename_model).layer(json_body_limit),
        )
        .route(
            "/koji/v1/models/:id/refresh",
            post(api::refresh_model_metadata).layer(json_body_limit),
        )
        .route(
            "/koji/v1/models/:id/verify",
            post(api::verify_model_files).layer(json_body_limit),
        )
        .route(
            "/koji/v1/models/:id/quants/:quant_key",
            delete(api::delete_quant),
        )
        .route(
            "/koji/v1/benchmarks/run",
            post(run_benchmark).layer(json_body_limit),
        )
        .route(
            "/koji/v1/downloads/:job_id/cancel",
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
                .allow_headers(tower_http::cors::Any),
        )
        .layer(middleware::from_fn(api::middleware::enforce_same_origin));

    Router::new()
        // Self-update GET routes (safe methods, no CSRF protection needed)
        .route(
            "/koji/v1/self-update/check",
            get(api::self_update::check_update),
        )
        .route(
            "/koji/v1/self-update/events",
            get(api::self_update::update_events),
        )
        // Benchmark GET routes (no CSRF needed)
        .route("/koji/v1/benchmarks/jobs/:id", get(get_benchmark_result))
        .route("/koji/v1/benchmarks/jobs/:id/events", get(benchmark_events))
        .route("/koji/v1/benchmarks/history", get(list_benchmark_history))
        .route("/koji/v1/benchmarks/history/:id", delete(delete_benchmark))
        // Downloads Center routes
        .route(
            "/koji/v1/downloads/active",
            get(api::downloads::get_active_downloads),
        )
        .route(
            "/koji/v1/downloads/history",
            get(api::downloads::get_download_history),
        )
        .route(
            "/koji/v1/downloads/events",
            get(api::downloads::download_events_sse),
        )
        // API documentation (OpenAPI 3.1.0 spec)
        .route("/koji/v1/docs", get(api::openapi::serve_spec))
        .merge(csrf_routes)
        .merge(backend_routes)
        .route("/koji/v1/*path", any(proxy_koji))
        .route("/", get(serve_index))
        .route(
            "/*path",
            get(|Path(p): Path<String>| async move { serve_static(Some(Path(p))).await }),
        )
        .layer(CatchPanicLayer::new())
        .with_state(state)
}

#[allow(clippy::too_many_arguments)]
pub async fn run_with_opts(
    addr: std::net::SocketAddr,
    proxy_base_url: String,
    logs_dir: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
    proxy_config: Option<Arc<tokio::sync::RwLock<koji_core::config::Config>>>,
    jobs: Option<Arc<JobManager>>,
    capabilities: Option<Arc<CapabilitiesCache>>,
    binary_version: String,
    download_queue: Option<Arc<DownloadQueueService>>,
) -> anyhow::Result<()> {
    let state = Arc::new(AppState {
        proxy_base_url,
        client: reqwest::Client::new(),
        logs_dir,
        config_path,
        proxy_config,
        jobs,
        capabilities,
        update_checker: Arc::new(koji_core::updates::UpdateChecker::new()),
        binary_version,
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        download_queue,
    });
    let jobs_for_shutdown = state.jobs.clone();
    let app = build_router(state);
    tracing::info!("Koji web UI listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(jobs_for_shutdown))
        .await?;
    Ok(())
}

/// Graceful shutdown signal handler.
/// Listens for SIGINT/SIGTERM and triggers cleanup:
/// - Kills all child processes
/// - Releases job manager active slots (SSE channels close when jobs are dropped)
async fn shutdown_signal(jobs: Option<Arc<JobManager>>) {
    // Wait for either SIGINT or SIGTERM
    #[cfg(unix)]
    {
        let sig_int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
        let sig_term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());

        match (sig_int, sig_term) {
            (Ok(mut int_signal), Ok(mut term_signal)) => {
                // Use select! to wait for either signal
                tokio::select! {
                    _ = int_signal.recv() => {
                        tracing::info!("Received SIGINT, shutting down gracefully...");
                    }
                    _ = term_signal.recv() => {
                        tracing::info!("Received SIGTERM, shutting down gracefully...");
                    }
                }
            }
            (Err(_), Ok(mut term_signal)) => {
                tracing::warn!("Unix signals not available, waiting for SIGTERM");
                let _ = term_signal.recv().await;
                tracing::info!("Received SIGTERM, shutting down gracefully...");
            }
            (Ok(mut int_signal), Err(_)) => {
                tracing::warn!("Unix signals not available, waiting for SIGINT");
                let _ = int_signal.recv().await;
                tracing::info!("Received SIGINT, shutting down gracefully...");
            }
            (Err(_), Err(_)) => {
                tracing::warn!("Unix signals not available, using ctrl_c fallback");
                tokio::signal::ctrl_c().await.ok();
                tracing::info!("Received interrupt, shutting down gracefully...");
            }
        }
    }
    #[cfg(not(unix))]
    {
        // On non-Unix platforms, use ctrl_c for graceful shutdown
        tracing::info!("Using Ctrl+C for graceful shutdown on this platform");
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Received interrupt, shutting down gracefully...");
    }

    // Cleanup: kill all child processes for active jobs
    if let Some(jobs) = jobs {
        if let Some(active_job) = jobs.active().await {
            tracing::info!("Killing children of active job {}...", active_job.id);
            jobs.kill_children(&active_job).await;
        }
    }
}

/// Convenience wrapper with no logs_dir/config_path.
pub async fn run(addr: std::net::SocketAddr, proxy_base_url: String) -> anyhow::Result<()> {
    run_with_opts(
        addr,
        proxy_base_url,
        None,
        None,
        None,
        None,
        None,
        env!("CARGO_PKG_VERSION").to_string(),
        None,
    )
    .await
}
