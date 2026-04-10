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
use tower_http::cors::CorsLayer;

use crate::api;
use crate::api::backends::{
    check_backend_updates, get_job, install_backend, job_events_sse, list_backends, remove_backend,
    system_capabilities, update_backend, update_backend_default_args, CapabilitiesCache,
};
use crate::jobs::JobManager;

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
async fn proxy_koji(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
    path: Path<String>,
    body: Body,
) -> Response {
    let url = format!("{}/koji/v1/{}", state.proxy_base_url, path.0);
    // Cap at 16 MiB — same as MAX_REQUEST_BODY_SIZE in koji-core — to prevent memory exhaustion.
    let body_bytes = axum::body::to_bytes(body, 16 * 1024 * 1024)
        .await
        .unwrap_or_default();

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
    // Build sub-router for backends API with origin enforcement and dedicated CORS
    let backend_routes = Router::new()
        .route("/api/system/capabilities", get(system_capabilities))
        .route("/api/backends", get(list_backends))
        .route("/api/backends/install", post(install_backend))
        .route("/api/backends/:name/update", post(update_backend))
        .route("/api/backends/:name", delete(remove_backend))
        .route(
            "/api/backends/:name/default-args",
            post(update_backend_default_args),
        )
        .route("/api/backends/check-updates", post(check_backend_updates))
        .route("/api/backends/jobs/:id", get(get_job))
        .route("/api/backends/jobs/:id/events", get(job_events_sse))
        .layer(middleware::from_fn(api::middleware::enforce_same_origin))
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::DELETE,
                ])
                .allow_headers(tower_http::cors::Any),
        );

    Router::new()
        .route("/api/logs", get(api::get_logs))
        .route("/api/config", get(api::get_config).post(api::save_config))
        .route(
            "/api/config/structured",
            get(api::get_structured_config).post(api::save_structured_config),
        )
        .route("/api/models", get(api::list_models).post(api::create_model))
        .route(
            "/api/models/:id",
            get(api::get_model)
                .put(api::update_model)
                .delete(api::delete_model),
        )
        .route("/api/models/:id/rename", post(api::rename_model))
        .route("/api/models/:id/refresh", post(api::refresh_model_metadata))
        .route("/api/models/:id/verify", post(api::verify_model_files))
        .route("/api/models/:id/quants/:quant_key", delete(api::delete_quant))
        .merge(backend_routes)
        .route("/koji/v1/*path", any(proxy_koji))
        .route("/", get(serve_index))
        .route(
            "/*path",
            get(|Path(p): Path<String>| async move { serve_static(Some(Path(p))).await }),
        )
        .with_state(state)
        .layer(CorsLayer::permissive())
}

pub async fn run_with_opts(
    addr: std::net::SocketAddr,
    proxy_base_url: String,
    logs_dir: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
    proxy_config: Option<Arc<tokio::sync::RwLock<koji_core::config::Config>>>,
    jobs: Option<Arc<JobManager>>,
    capabilities: Option<Arc<CapabilitiesCache>>,
) -> anyhow::Result<()> {
    let state = Arc::new(AppState {
        proxy_base_url,
        client: reqwest::Client::new(),
        logs_dir,
        config_path,
        proxy_config,
        jobs,
        capabilities,
    });
    let app = build_router(state);
    tracing::info!("Koji web UI listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Convenience wrapper with no logs_dir/config_path.
pub async fn run(addr: std::net::SocketAddr, proxy_base_url: String) -> anyhow::Result<()> {
    run_with_opts(addr, proxy_base_url, None, None, None, None, None).await
}
