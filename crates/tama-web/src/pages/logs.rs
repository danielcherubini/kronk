use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::utils::extract_and_store_csrf_token;

/// Response from GET /tama/v1/logs — grouped by source.
#[derive(Debug, Clone, Deserialize)]
struct AllLogsResponse {
    sources: Vec<SourceLogs>,
}

/// Logs for a single source (e.g. "tama", "llama_cpp_1").
#[derive(Debug, Clone, Deserialize)]
struct SourceLogs {
    name: String,
    lines: Vec<String>,
}

/// Classify a log line and return the CSS modifier class suffix.
fn log_level_class(line: &str) -> &'static str {
    let upper = line.to_uppercase();
    if upper.contains("ERROR") || upper.contains("FATAL") {
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
    // Selected source (empty = show all)
    let selected_source = RwSignal::new(String::new());

    // Log data grouped by source
    let sources = RwSignal::new(Vec::<SourceLogs>::new());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load all logs from the API
    let load_logs = move || {
        spawn_local(async move {
            loading.set(true);
            error.set(None);

            match Request::get("/tama/v1/logs").send().await {
                Ok(resp) => {
                    extract_and_store_csrf_token(&resp);
                    let status = resp.status();
                    if status >= 200 && status < 300 {
                        match resp.text().await {
                            Ok(text) => match serde_json::from_str::<AllLogsResponse>(&text) {
                                Ok(data) => sources.set(data.sources),
                                Err(e) => error.set(Some(format!(
                                    "Parse error: {e} (body len={})",
                                    text.len()
                                ))),
                            },
                            Err(e) => error.set(Some(format!("Failed to read body: {e}"))),
                        }
                    } else {
                        error.set(Some(format!(
                            "HTTP {} — logs_dir may not be configured",
                            resp.status()
                        )));
                        sources.set(Vec::new());
                    }
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load logs: {e}")));
                    sources.set(Vec::new());
                }
            }
            loading.set(false);
        });
    };

    // Load on mount, then every 5 seconds
    Effect::new(move |_| {
        load_logs();
        spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(5_000).await;
                load_logs();
            }
        });
    });

    // Check for ?source= query parameter on mount and pre-select it
    Effect::new(move |_| {
        if let Some(href) = web_sys::window().and_then(|w| w.location().href().ok()) {
            if let Some(query_start) = href.find('?') {
                let query = &href[query_start + 1..];
                for param in query.split('&') {
                    if let Some(eq_pos) = param.find('=') {
                        let key = &param[..eq_pos];
                        let value = urlencoding::decode(&param[eq_pos + 1..]).ok();
                        if key == "source" {
                            if let Some(source) = value {
                                if !source.is_empty() {
                                    selected_source.set(source.to_string());
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
                    prop:value=selected_source
                    on:change=move |e| {
                        let val = e
                            .target()
                            .unwrap()
                            .dyn_into::<web_sys::HtmlSelectElement>()
                            .unwrap();
                        selected_source.set(val.value());
                    }
                >
                    <option value="">"All Sources"</option>
                    {move || {
                        sources.get().into_iter().map(|s| {
                            let name = s.name.clone();
                            view! {
                                <option value=name.clone()> {name.clone()} </option>
                            }.into_any()
                        }).collect::<Vec<_>>()
                    }}
                </select>
                <button
                    class="btn btn-secondary btn-sm"
                    prop:disabled=loading.get()
                    on:click=move |_| { load_logs(); }
                >
                    "↻ Refresh"
                </button>
            </div>
        </div>

        // Loading state
        {move || {
            let all_sources = sources.get();
            let err = error.get();
            let is_loading = loading.get();
            if is_loading && all_sources.is_empty() {
                view! {
                    <div class="spinner-container mt-4">
                        <span class="spinner"></span>
                        <span class="text-muted">"Loading logs..."</span>
                    </div>
                }.into_any()
            } else if let Some(e) = err {
                view! {
                    <div class="alert alert--warning mt-2">
                        <span class="alert__icon">"⚠"</span>
                        <span>{e}</span>
                    </div>
                }.into_any()
            } else if all_sources.is_empty() {
                view! {
                    <div class="alert alert--info mt-2">
                        <span class="alert__icon">"ℹ"</span>
                        <span>"No logs yet. Logs will appear here after backend processes are started."</span>
                    </div>
                }.into_any()
            } else {
                let selected = selected_source.get();
                let selected_clone = selected.clone();
                view! {
                    <div class="log-viewer card">
                        {all_sources.into_iter().filter(move |s| {
                            selected.is_empty() || s.name == selected
                        }).flat_map(|source| {
                            // Add a header for each source (unless showing all)
                            let headers = if selected_clone.is_empty() && !source.lines.is_empty() {
                                vec![format!("=== {} ===", source.name)]
                            } else {
                                vec![]
                            };
                            let lines = headers.into_iter().chain(source.lines).collect::<Vec<_>>();
                            lines.into_iter().map(|line| {
                                let cls = if line.starts_with("===") {
                                    "log-line log-line--header".to_string()
                                } else {
                                    format!("log-line {}", log_level_class(&line))
                                };
                                view! { <div class=cls>{line}</div> }
                            }).collect::<Vec<_>>()
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }
        }}
    }
}
