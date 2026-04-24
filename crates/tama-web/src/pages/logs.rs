use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::utils::extract_and_store_csrf_token;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogsResponse {
    lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackendInfo {
    name: String,
    display_name: Option<String>,
    version: Option<String>,
}

/// Classify a log line and return the CSS modifier class suffix.
fn log_level_class(line: &str) -> &'static str {
    let upper = line.to_uppercase();
    if upper.contains("ERROR") {
        "log-line--error"
    } else if upper.contains("WARN") {
        "log-line--warn"
    } else if upper.contains("DEBUG") {
        "log-line--debug"
    } else {
        "log-line--info"
    }
}

#[component]
pub fn Logs() -> impl IntoView {
    // Selected backend (empty = show all logs)
    let selected_backend = RwSignal::new(String::new());

    // List of backends with running models
    let backends = RwSignal::new(Vec::<BackendInfo>::new());
    let loading_backends = RwSignal::new(false);

    // Log data
    let log_lines = RwSignal::new(Vec::<String>::new());
    let loading_logs = RwSignal::new(false);
    let log_error = RwSignal::new(Option::<String>::None);

    // Load the list of backends with running models
    let load_backends = move || {
        spawn_local(async move {
            loading_backends.set(true);
            match Request::get("/tama/v1/system/capabilities")
                .send()
                .await
            {
                Ok(resp) => {
                    extract_and_store_csrf_token(&resp);
                    if let Ok(info) = resp.json::<serde_json::Value>().await {
                        if let Some(models) = info.get("models").and_then(|v| v.as_array()) {
                            let mut seen: std::collections::HashSet<String> =
                std::collections::HashSet::new();
                            let mut result = Vec::new();
                            for m in models {
                                if let (Some(backend), Some(id)) = (
                                    m.get("backend").and_then(|v| v.as_str()),
                                    m.get("id").and_then(|v| v.as_str()),
                                ) {
                                    let key = format!("{}:{}", backend, id);
                                    if seen.insert(key) {
                                        result.push(BackendInfo {
                                            name: format!("{}_{}", backend, id),
                                            display_name: m.get("display_name")
                                                .and_then(|v| v.as_str())
                                                .map(String::from),
                                            version: m.get("version")
                                                .and_then(|v| v.as_str())
                                                .map(String::from),
                                        });
                                    }
                                }
                            }
                            backends.set(result);
                        }
                    }
                }
                Err(_e) => {}
                // Silently ignore — backends list will retry on next interval
            }
            loading_backends.set(false);
        });
    };

    // Load logs for the selected backend (or all logs if none selected)
    let load_logs = move || {
        spawn_local(async move {
            let backend = selected_backend.get();
            let url = if backend.is_empty() {
                "/tama/v1/logs".to_string()
            } else {
                format!("/tama/v1/logs/{}?lines=2000", backend)
            };

            loading_logs.set(true);
            log_error.set(None);

            match Request::get(&url).send().await {
                Ok(resp) => {
                    extract_and_store_csrf_token(&resp);
                    if resp.status() >= 200 && resp.status() < 300 {
                        if let Ok(data) = resp.json::<LogsResponse>().await {
                            log_lines.set(data.lines);
                        } else {
                            log_error.set(Some("Failed to parse log data".to_string()));
                        }
                    } else {
                        log_error.set(Some(format!(
                            "HTTP {} — backend may not be running",
                            resp.status()
                        )));
                        log_lines.set(Vec::new());
                    }
                }
                Err(e) => {
                    log_error.set(Some(format!("Failed to load logs: {e}")));
                    log_lines.set(Vec::new());
                }
            }
            loading_logs.set(false);
        });
    };

    // Load backends on mount, then every 10 seconds
    Effect::new(move |_| {
        load_backends();
        spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(10_000).await;
                load_backends();
            }
        });
    });

    // Load logs when backend selection changes or on initial mount
    Effect::new(move |_| {
        load_logs();
    });

    // Check for ?backend= query parameter on mount and pre-select it
    Effect::new(move |_| {
        if let Some(href) = web_sys::window().and_then(|w| w.location().href().ok()) {
            if let Some(query_start) = href.find('?') {
                let query = &href[query_start + 1..];
                for param in query.split('&') {
                    if let Some(eq_pos) = param.find('=') {
                        let key = &param[..eq_pos];
                        let value = urlencoding::decode(&param[eq_pos + 1..]).ok();
                        if key == "backend" {
                            if let Some(backend) = value {
                                if !backend.is_empty() && backends.get().iter().any(|b| b.name == backend) {
                                    selected_backend.set(backend.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    view! {
        <div class="page-header">
            <h1>"Log Viewer"</h1>
            <div class="log-toolbar">
                <select
                    class="form-select form-select-sm"
                    prop:value=selected_backend
                    on:change=move |e| {
                        let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap();
                        selected_backend.set(val.value());
                        load_logs();
                    }
                >
                    <option value="">"All Logs"</option>
                    {move || {
                        backends.get().into_iter().map(|b| {
                            let label = b.display_name
                                .as_deref()
                                .or(Some(b.name.as_str()))
                                .unwrap_or(&b.name);
                            let version_tag = b.version.as_deref().map(|v| format!(" ({})", v)).unwrap_or_default();
                            view! {
                                <option value=b.name.clone()>{format!("{}{}", label, version_tag)}</option>
                            }.into_any()
                        }).collect::<Vec<_>>()
                    }}
                </select>
                <button
                    class="btn btn-secondary btn-sm"
                    prop:disabled=loading_logs.get()
                    on:click=move |_| { load_logs(); }
                >
                    "↻ Refresh"
                </button>
            </div>
        </div>

        // Loading state
        {if loading_logs.get() && log_lines.get().is_empty() {
            view! {
                <div class="spinner-container mt-4">
                    <span class="spinner"></span>
                    <span class="text-muted">"Loading logs..."</span>
                </div>
            }.into_any()
        } else if let Some(err) = log_error.get() {
            view! {
                <div class="alert alert--warning mt-2">
                    <span class="alert__icon">"⚠"</span>
                    <span>{err}</span>
                </div>
            }.into_any()
        } else if log_lines.get().is_empty() {
            view! {
                <div class="alert alert--info mt-2">
                    <span class="alert__icon">"ℹ"</span>
                    <span>"No logs yet. Logs will appear here after backend processes are started."</span>
                </div>
            }.into_any()
        } else {
            let lines = log_lines.get();
            view! {
                <div class="log-viewer card">
                    {lines.into_iter().map(|line| {
                        let level_cls = log_level_class(&line);
                        let cls = format!("log-line {}", level_cls);
                        view! { <div class=cls>{line}</div> }
                    }).collect::<Vec<_>>()}
                </div>
            }.into_any()
        }}
    }
}
