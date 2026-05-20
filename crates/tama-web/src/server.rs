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
    benchmark_events, delete_benchmark, get_benchmark_result, list_benchmark_history,
    run_benchmark, run_mtp_benchmark, run_spec_benchmark,
};
use crate::jobs::JobManager;
#[allow(unused_imports)]
use tama_core::proxy::download_queue::DownloadQueueService;

static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/dist");

#[derive(Clone)]
pub struct AppState {
    pub proxy_base_url: String,
    pub client: reqwest::Client,
    pub logs_dir: Option<std::path::PathBuf>,
    pub config_path: Option<std::path::PathBuf>,
    pub proxy_config: Option<Arc<tokio::sync::RwLock<tama_core::config::Config>>>,
    pub jobs: Option<Arc<JobManager>>,
    pub capabilities: Option<Arc<CapabilitiesCache>>,
    /// Shared update checker to prevent concurrent runs across requests.
    pub update_checker: Arc<tama_core::updates::UpdateChecker>,
    /// The version of the running tama binary (passed from the CLI at startup).
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
            .unwrap_or_else(|| std::env::temp_dir().join("tama_uploads"))
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

/// Forward a request to the Tama proxy at `/tama/v1/<path>`.
/// Only allows GET, POST, and PATCH methods; returns 405 for others.
async fn proxy_tama(
    State(state): State<Arc<tama_core::proxy::ProxyState>>,
    method: Method,
    headers: HeaderMap,
    path: Path<String>,
    body: Body,
) -> Response {
    // Whitelist allowed methods
    if !matches!(method, Method::GET | Method::POST | Method::PATCH) {
        return (StatusCode::METHOD_NOT_ALLOWED, "Method not allowed").into_response();
    }

    let proxy_url = state.config.read().await.proxy_url();
    let url = format!("{}/tama/v1/{}", proxy_url, path.0);
    // Cap at 16 MiB — same as MAX_REQUEST_BODY_SIZE in tama-core — to prevent memory exhaustion.
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
                // Skip hop-by-hop headers that shouldn't be forwarded
                if k.as_str().eq_ignore_ascii_case("connection")
                    || k.as_str().eq_ignore_ascii_case("keep-alive")
                    || k.as_str().eq_ignore_ascii_case("transfer-encoding")
                {
                    continue;
                }
                response.headers_mut().insert(k, v.clone());
            }
            // Ensure SSE connections stay open — tell browser not to cache
            if is_sse {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    "no-cache".parse().unwrap(),
                );
                response.headers_mut().insert(
                    axum::http::header::CONNECTION,
                    "keep-alive".parse().unwrap(),
                );
            }
            response
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("Failed to reach Tama proxy: {e}"),
        )
            .into_response(),
    }
}

/// Dedicated handler for the root path — avoids Axum type-inference issues with inline closures.
async fn serve_index() -> Response {
    serve_static(None).await
}

pub fn build_router(state: Arc<tama_core::proxy::ProxyState>) -> Router {
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
        // HF metadata endpoint — must come before catch-all proxy_tama
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
        // Logs: /tama/v1/logs falls through to proxy_tama catch-all (real handler in tama-core)
        // Only /:backend has a local file-based fallback
        .route("/tama/v1/logs/:backend", get(api::logs::get_backend_logs))
        .merge(csrf_routes)
        .merge(backend_routes)
        .route("/tama/v1/*path", any(proxy_tama))
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
    _proxy_base_url: String,
    _logs_dir: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
    proxy_config: Option<Arc<tokio::sync::RwLock<tama_core::config::Config>>>,
    jobs: Option<Arc<tama_core::web_types::JobManager>>,
    capabilities: Option<Arc<tama_core::web_types::CapabilitiesCache>>,
    binary_version: String,
    download_queue: Option<Arc<DownloadQueueService>>,
    shutdown_rx: Option<tokio::sync::watch::Receiver<()>>,
) -> anyhow::Result<()> {
    // Build config from proxy_config or load from disk
    let config = if let Some(ref pc) = proxy_config {
        pc.read().await.clone()
    } else if let Some(ref cp) = config_path {
        let config_dir = cp.parent().map(|p| p.to_path_buf());
        if let Some(cd) = config_dir {
            tama_core::config::Config::load_from(&cd).unwrap_or_default()
        } else {
            tama_core::config::Config::default()
        }
    } else {
        tama_core::config::Config::default()
    };

    // Derive db_dir from config_path
    let db_dir = config_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());

    let state = Arc::new(tama_core::proxy::ProxyState::new(config, db_dir.clone()));

    // Set web-specific fields
    let mut state_inner = (*state).clone();
    state_inner.web_jobs = jobs.clone();
    state_inner.web_capabilities = capabilities.clone();
    state_inner.web_binary_version = binary_version;
    if let Some(ref dq) = download_queue {
        state_inner.download_queue = Some(dq.clone());
    }
    let state = Arc::new(state_inner);

    let jobs_for_shutdown = state.web_jobs.clone();
    let app = build_router(state);
    tracing::info!("Tama web UI listening on http://{}", addr);

    // Use axum-server for timeout-based graceful shutdown.
    // SSE streams (metrics, jobs, downloads) can hold connections open
    // indefinitely — the timeout forces them closed after 5s.
    use axum_server::Handle;
    let handle = Handle::new();
    let shutdown_handle = handle.clone();

    tokio::spawn(async move {
        shutdown_signal_inner(jobs_for_shutdown, shutdown_rx).await;
        tracing::info!("Web UI initiating graceful shutdown (timeout: 5s)...");
        shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
    });

    // axum_server::from_tcp takes a std::net::TcpListener
    let std_listener = std::net::TcpListener::bind(addr)?;
    std_listener.set_nonblocking(true)?;

    let result = axum_server::from_tcp(std_listener)
        .handle(handle)
        .serve(app.into_make_service())
        .await;

    result?;
    Ok(())
}

/// Wait for the shutdown trigger (shared channel or own signal handlers).
/// Does NOT do cleanup here — cleanup is done by the caller after this returns.
async fn shutdown_signal_inner(
    jobs: Option<Arc<tama_core::web_types::JobManager>>,
    shutdown_rx: Option<tokio::sync::watch::Receiver<()>>,
) {
    if let Some(mut rx) = shutdown_rx {
        // Wait for the shutdown signal from the proxy server
        let _ = rx.changed().await;
        tracing::info!("Shutdown signal received, shutting down web UI...");
    } else {
        // Standalone mode — register our own signal handlers
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            if let (Ok(mut sigint), Ok(mut sigterm)) = (
                signal(SignalKind::interrupt()),
                signal(SignalKind::terminate()),
            ) {
                tokio::select! {
                    _ = sigint.recv() => {
                        tracing::info!("Received SIGINT, shutting down web UI...");
                    }
                    _ = sigterm.recv() => {
                        tracing::info!("Received SIGTERM, shutting down web UI...");
                    }
                }
            } else {
                tokio::signal::ctrl_c().await.ok();
                tracing::info!("Received interrupt, shutting down web UI...");
            }
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("Received interrupt, shutting down web UI...");
        }
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
/// Runs in standalone mode (registers own signal handlers).
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
        None,
    )
    .await
}
