use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogsResponse {
    lines: Vec<String>,
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
    let refresh = RwSignal::new(0u32);

    let logs = LocalResource::new(move || async move {
        let _ = refresh.get(); // track the signal
        let resp = gloo_net::http::Request::get("/koji/v1/logs")
            .send()
            .await
            .ok()?;
        resp.json::<LogsResponse>().await.ok()
    });

    view! {
        <div class="page-header">
            <h1>"Log Viewer"</h1>
            <div class="log-toolbar">
                <button
                    class="btn btn-secondary btn-sm"
                    on:click=move |_| { refresh.update(|n| *n += 1); }
                >
                    "↻ Refresh"
                </button>
            </div>
        </div>

        <Suspense fallback=|| view! {
            <div class="spinner-container">
                <span class="spinner"></span>
                <span class="text-muted">"Loading logs..."</span>
            </div>
        }>
            {move || {
                logs.get().map(|guard| {
                    let result = guard.take();
                    match result {
                        Some(data) => {
                            let lines = data.lines.clone();
                            if lines.is_empty() {
                                view! {
                                    <div class="alert alert--info mt-2">
                                        <span class="alert__icon">"ℹ"</span>
                                        <span>"No logs yet. Logs will appear here after backend processes are started."</span>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="log-viewer card">
                                        {lines.into_iter().map(|line| {
                                            let level_cls = log_level_class(&line);
                                            let cls = format!("log-line {}", level_cls);
                                            view! { <div class=cls>{line}</div> }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                }.into_any()
                            }
                        }
                        None => view! {
                            <div class="alert alert--warning mt-2">
                                <span class="alert__icon">"⚠"</span>
                                <span>"Failed to load logs (is " <code>"logs_dir"</code> " configured?)"</span>
                            </div>
                        }.into_any(),
                    }
                })
            }}
        </Suspense>
    }
}
