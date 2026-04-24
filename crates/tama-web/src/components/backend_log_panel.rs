//! Backend log panel - displays live backend logs via HTTP polling.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

use crate::utils::extract_and_store_csrf_token;

/// Response structure from the backend logs API.
#[derive(Debug, Clone, Deserialize)]
struct LogsResponse {
    lines: Vec<String>,
}

const MAX_LINES: usize = 1000;
const POLL_INTERVAL_MS: u32 = 1000;

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

/// Fetch log lines from the backend API for a given backend name.
async fn fetch_logs(
    backend: &str,
    lines: RwSignal<Vec<String>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) {
    let url = format!("/tama/v1/logs/{}", backend);
    match Request::get(&url).send().await {
        Ok(resp) => {
            extract_and_store_csrf_token(&resp);

            if resp.status() == 200 {
                if let Ok(data) = resp.json::<LogsResponse>().await {
                    lines.update(|v| {
                        v.clear();
                        v.extend(data.lines);
                        if v.len() > MAX_LINES {
                            let drop_count = v.len() - MAX_LINES;
                            v.drain(0..drop_count);
                        }
                    });
                    loading.set(false);
                    error.set(None);
                } else {
                    error.set(Some("Failed to parse log response".to_string()));
                    loading.set(false);
                }
            } else if resp.status() == 404 {
                error.set(Some(format!("No logs found for '{}'", backend)));
                lines.update(|v| v.clear());
                loading.set(false);
            } else {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                error.set(Some(format!(
                    "Server responded with status {}: {}",
                    status, body
                )));
                loading.set(false);
            }
        }
        Err(e) => {
            let msg = e.to_string();
            error.set(Some(format!("Network error: {}", msg)));
            loading.set(false);
        }
    }
}

/// BackendLogPanel - polls the backend log API for live-updating log display.
#[component]
pub fn BackendLogPanel(
    /// The backend name whose logs we should display
    backend_name: String,
    /// Called when user clicks Close or X
    #[prop(optional)]
    on_close: Option<Callback<()>>,
) -> impl IntoView {
    // Clone for reuse in closures and view.
    let backend_display = backend_name.clone();

    let lines = RwSignal::new(Vec::<String>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let auto_refresh = RwSignal::new(true);
    let last_fetch = RwSignal::new(std::time::Instant::now());

    // Cancel flag: flipped on component unmount. The spawned async tasks check
    // this each iteration and break out cleanly.
    let cancelled = RwSignal::new(false);
    on_cleanup(move || {
        cancelled.set(true);
    });

    // Spawn initial fetch on mount (without auto-refresh).
    {
        let backend = backend_name.clone();
        let cancel = cancelled;
        spawn_local(async move {
            if !cancel.get_untracked() {
                fetch_logs(&backend, lines, loading, error).await;
            }
        });
    }

    // Auto-refresh effect: runs every frame, checks if auto_refresh is true and
    // if enough time has elapsed since last fetch.
    let backend_for_effect = backend_name.clone();
    Effect::new(move |_| {
        let backend = backend_for_effect.clone();
        let cancel = cancelled;
        let lines_for_poll = lines;
        let loading_for_poll = loading;
        let error_for_poll = error;
        let auto_refresh_for_poll = auto_refresh;
        let last_fetch_for_poll = last_fetch;

        spawn_local(async move {
            loop {
                // Wait for poll interval.
                gloo_timers::future::TimeoutFuture::new(POLL_INTERVAL_MS).await;

                if cancel.get_untracked() {
                    return;
                }

                if !auto_refresh_for_poll.get_untracked() {
                    continue;
                }

                fetch_logs(&backend, lines_for_poll, loading_for_poll, error_for_poll).await;
                last_fetch_for_poll.set(std::time::Instant::now());
            }
        });
    });

    let on_close_handler = move |_| {
        if let Some(cb) = &on_close {
            cb.run(());
        }
    };

    // Clone for the refresh handler (view! macro also captures backend_display).
    let backend_for_refresh = backend_display.clone();
    let on_refresh_handler = move |_| {
        let backend = backend_for_refresh.clone();
        let cancel = cancelled;
        let lines_ref = lines;
        let loading_ref = loading;
        let error_ref = error;
        let last_fetch_ref = last_fetch;
        spawn_local(async move {
            if !cancel.get_untracked() {
                fetch_logs(&backend, lines_ref, loading_ref, error_ref).await;
                last_fetch_ref.set(std::time::Instant::now());
            }
        });
    };

    let on_toggle_refresh = move |_| {
        auto_refresh.update(|v| *v = !*v);
    };

    view! {
        <div
            style="border:1px solid var(--border,#ccc);border-radius:6px;background:#0f172a;color:#e2e8f0;font-family:monospace;font-size:0.75rem;max-height:400px;display:flex;flex-direction:column;"
        >
            // Header bar
            <div style="display:flex;justify-content:space-between;align-items:center;padding:0.5rem 0.75rem;background:#1e293b;border-bottom:1px solid #334155;">
                <div style="font-weight:600;font-size:0.875rem;">"📋 " {move || backend_display.clone()} " logs"</div>
                <div style="display:flex;gap:0.5rem;align-items:center;">
                    <button
                        type="button"
                        class="btn btn-sm btn-secondary"
                        on:click=on_refresh_handler
                        title="Refresh now"
                    >
                        "↻ Refresh"
                    </button>
                    <button
                        type="button"
                        class="btn btn-sm btn-secondary"
                        on:click=on_toggle_refresh
                        title="Toggle auto-refresh"
                    >
                        {move || if auto_refresh.get() { "Pause" } else { "Resume" }}
                    </button>
                    <button
                        type="button"
                        class="btn btn-sm btn-secondary"
                        on:click=on_close_handler
                        title="Close"
                        style="font-weight:bold;"
                    >
                        "×"
                    </button>
                </div>
            </div>

            // Content area
            <div style="overflow-y:auto;padding:0.5rem 0.75rem;flex:1;">
                {move || {
                    if let Some(err) = error.get() {
                        view! {
                            <div class="log-line log-line--error">{err}</div>
                        }.into_any()
                    } else if loading.get() {
                        view! {
                            <div style="color:#94a3b8;">"Loading logs..."</div>
                        }.into_any()
                    } else {
                        let all_lines = lines.get();
                        if all_lines.is_empty() {
                            view! {
                                <div style="color:#94a3b8;">"No logs yet..."</div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="log-panel">
                                    {all_lines.into_iter().map(|line| {
                                        let level_cls = log_level_class(&line);
                                        let cls = format!("log-line {}", level_cls);
                                        view! { <div class=cls>{line}</div> }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        }
                    }
                }}
            </div>
        </div>
    }
}
