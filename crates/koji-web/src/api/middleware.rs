use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

/// Enforce same-origin for state-changing methods (POST, DELETE, etc.).
///
/// - GET/HEAD/OPTIONS: pass through
/// - POST/DELETE: check Origin header matches Host header
///   - If Origin present and doesn't match: return 403
///   - If Origin absent: allow through (same-origin fetch from form, server-to-server, or curl)
pub async fn enforce_same_origin(
    req: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let method = req.method().clone();

    // Allow safe methods without Origin check
    if matches!(
        method,
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    ) {
        return Ok(next.run(req).await);
    }

    // For state-changing methods, check Origin if present
    if let Some(origin) = req.headers().get(axum::http::header::ORIGIN) {
        if let Some(host) = req.headers().get(axum::http::header::HOST) {
            let origin_str = origin.to_str().unwrap_or("");
            let host_str = host.to_str().unwrap_or("");

            // Compare origin with host (origin includes scheme, host does not)
            // For same-origin, origin should be http://host or https://host
            let expected_origin = format!("http://{}", host_str);
            let expected_origin_ssl = format!("https://{}", host_str);

            if origin_str != expected_origin && origin_str != expected_origin_ssl {
                return Err((StatusCode::FORBIDDEN, "Cross-origin requests not allowed"));
            }
        } else {
            // Origin present but HOST missing — reject state-changing methods
            return Err((StatusCode::FORBIDDEN, "Cross-origin requests not allowed"));
        }
    }

    // Origin absent or matches — allow through
    Ok(next.run(req).await)
}
