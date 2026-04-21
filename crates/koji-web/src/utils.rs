pub mod self_update;

use gloo_net::http::{Request, RequestBuilder, Response};
use wasm_bindgen::JsValue;

/// CSRF token cookie name — must match server-side constant.
const CSRF_COOKIE_NAME: &str = "koji_csrf_token";

/// Key for storing CSRF token in sessionStorage (fallback when cookie is unavailable).
const CSRF_STORAGE_KEY: &str = "_koji_csrf_token";

/// Read the CSRF token from document.cookie using js_sys Reflect.
pub fn get_csrf_cookie() -> Option<String> {
    let doc = web_sys::window()?.document()?;
    let cookie: String = js_sys::Reflect::get(&doc, &JsValue::from_str("cookie"))
        .ok()?
        .as_string()?;
    for part in cookie.split(';') {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            if key == CSRF_COOKIE_NAME {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Read the CSRF token from localStorage (fallback when cookie is unavailable).
pub fn get_csrf_stored() -> Option<String> {
    let storage = web_sys::window()?.local_storage().ok()??;
    storage.get_item(CSRF_STORAGE_KEY).ok().flatten()
}

/// Store a CSRF token in localStorage for fallback use.
pub fn store_csrf_token(token: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(CSRF_STORAGE_KEY, token);
    }
}

/// Get the CSRF token, trying cookie first then sessionStorage fallback.
pub fn get_csrf_token() -> Option<String> {
    get_csrf_cookie().or_else(get_csrf_stored)
}

/// Extract and store the CSRF token from a GET response header.
/// The middleware sets `X-CSRF-Token` on all responses (GET, POST, etc.).
pub fn extract_and_store_csrf_token(resp: &Response) {
    if let Some(token) = resp.headers().get("X-CSRF-Token") {
        let token_str = token.as_str();
        store_csrf_token(token_str);
    }
}

/// Build a POST request with X-CSRF-Token header injected.
pub fn post_request(url: &str) -> RequestBuilder {
    let mut builder = Request::post(url);
    if let Some(token) = get_csrf_token() {
        builder = builder.header("X-CSRF-Token", &token);
    }
    builder
}

/// Build a PUT request with X-CSRF-Token header injected.
pub fn put_request(url: &str) -> RequestBuilder {
    let mut builder = Request::put(url);
    if let Some(token) = get_csrf_token() {
        builder = builder.header("X-CSRF-Token", &token);
    }
    builder
}

/// Format bytes into a human-readable string (KB, MB, GB).
pub fn format_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Helper to convert event.target.value as String
/// Handles input, select, and textarea elements uniformly
pub fn target_value(ev: &leptos::ev::Event) -> String {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| {
            t.dyn_into::<web_sys::HtmlInputElement>()
                .ok()
                .map(|i| i.value())
                .or_else(|| {
                    ev.target().and_then(|t| {
                        t.dyn_into::<web_sys::HtmlSelectElement>()
                            .ok()
                            .map(|s| s.value())
                    })
                })
                .or_else(|| {
                    ev.target().and_then(|t| {
                        t.dyn_into::<web_sys::HtmlTextAreaElement>()
                            .ok()
                            .map(|s| s.value())
                    })
                })
        })
        .unwrap_or_default()
}
