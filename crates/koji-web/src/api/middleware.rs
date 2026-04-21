use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

/// Determine whether the Secure cookie flag should be set.
/// Only sets Secure when we can confirm HTTPS — either via X-Forwarded-Proto header
/// (when behind a TLS-terminating proxy) or when the host is explicitly known to use HTTPS.
/// Defaults to false for all other cases: setting Secure without confirming HTTPS causes
/// the browser to silently drop the cookie on HTTP connections, breaking CSRF protection.
fn should_set_secure(headers: &axum::http::HeaderMap) -> bool {
    // Check X-Forwarded-Proto first (set by reverse proxies like nginx, Caddy)
    if let Some(forwarded_proto) = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
    {
        return forwarded_proto.starts_with("https");
    }

    // No proxy header — we can't confirm HTTPS, so don't set Secure.
    // This avoids the common issue where non-localhost hosts get a Secure cookie
    // that browsers refuse to send over plain HTTP.
    false
}

/// CSRF token cookie name.
const CSRF_COOKIE_NAME: &str = "koji_csrf_token";
/// CSRF token header name expected on state-changing requests.
const CSRF_HEADER_NAME: &str = "X-CSRF-Token";

/// Generate a cryptographically random CSRF token (32 bytes, hex-encoded).
fn generate_csrf_token() -> String {
    // Use uuid v4 for randomness; encode as hex string (no dashes)
    let id = uuid::Uuid::new_v4();
    let (hi, lo) = id.as_u64_pair();
    format!("{:x}{:x}", hi, lo)
}

/// Enforce same-origin for state-changing methods (POST, DELETE, etc.).
///
/// - GET/HEAD/OPTIONS: generate and set CSRF token (cookie + header)
/// - POST/PUT/PATCH: verify CSRF double-submit (cookie matches header)
/// - DELETE: check Origin header matches Host header (legacy fallback)
pub async fn enforce_same_origin(
    req: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let method = req.method().clone();

    // Safe methods: generate CSRF token and set cookie + header
    if matches!(
        method,
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    ) {
        let token = generate_csrf_token();

        // Determine if Secure flag should be set (only when HTTPS confirmed via X-Forwarded-Proto)
        let is_secure = should_set_secure(req.headers());

        // Build cookie string — NO HttpOnly so JS can read it for CSRF double-submit.
        // Secure flag is conditional: only set on non-localhost hosts (HTTPS).
        let secure_attr = if is_secure { "; Secure" } else { "" };
        let set_cookie = format!(
            "{}={}; Path=/; SameSite=Lax{}",
            CSRF_COOKIE_NAME, token, secure_attr
        );

        let mut response = next.run(req).await;
        response
            .headers_mut()
            .insert(axum::http::header::SET_COOKIE, set_cookie.parse().unwrap());
        response
            .headers_mut()
            .insert(CSRF_HEADER_NAME, token.parse().unwrap());
        return Ok(response);
    }

    // POST/PUT/PATCH: verify CSRF double-submit (cookie + header must match)
    if matches!(
        method,
        axum::http::Method::POST | axum::http::Method::PUT | axum::http::Method::PATCH
    ) {
        let cookie_header = req
            .headers()
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Extract CSRF token from cookie
        let cookie_token = extract_csrf_cookie(cookie_header);

        // Get CSRF token from header
        let header_token = req
            .headers()
            .get(CSRF_HEADER_NAME)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        match (cookie_token, header_token) {
            (Some(cookie_val), Some(header_val)) if cookie_val == header_val => {
                // Tokens match — allow through
                Ok(next.run(req).await)
            }
            _ => {
                // Missing or mismatched CSRF token
                Err((StatusCode::FORBIDDEN, "CSRF token validation failed"))
            }
        }
    } else if matches!(method, axum::http::Method::DELETE) {
        // DELETE: check Origin if present (legacy fallback for non-POST methods)
        if let Some(origin) = req.headers().get(axum::http::header::ORIGIN) {
            if let Some(host) = req.headers().get(axum::http::header::HOST) {
                let origin_str = origin.to_str().unwrap_or("");
                let host_str = host.to_str().unwrap_or("");

                let expected_origin = format!("http://{}", host_str);
                let expected_origin_ssl = format!("https://{}", host_str);

                if origin_str != expected_origin && origin_str != expected_origin_ssl {
                    return Err((StatusCode::FORBIDDEN, "Cross-origin requests not allowed"));
                }
            } else {
                return Err((StatusCode::FORBIDDEN, "Cross-origin requests not allowed"));
            }
        }
        Ok(next.run(req).await)
    } else {
        // Other methods pass through
        Ok(next.run(req).await)
    }
}

/// Extract CSRF token value from a Cookie header string.
fn extract_csrf_cookie(cookie_header: &str) -> Option<String> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            if key == CSRF_COOKIE_NAME {
                return Some(value.to_string());
            }
        }
    }
    None
}
