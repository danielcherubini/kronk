//! Backend log panel - displays live backend logs via SSE streaming.

use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

/// Response structure from the backend logs API (for non-streaming fallback).
#[derive(Debug, Clone, Deserialize)]
struct LogsResponse {
    lines: Vec<String>,
}

/// SSE event payload.
#[derive(Debug, Clone, Deserialize)]
struct LogPayload {
    line: String,
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

/// BackendLogPanel - subscribes to SSE stream of backend log lines.
#[component]
pub fn BackendLogPanel(
    /// The backend/server identifier whose logs we should display.
    backend_name: String,
    /// Called when user clicks Close or X.
    #[prop(optional)]
    on_close: Option<Callback<()>>,
) -> impl IntoView {
    let lines = RwSignal::new(Vec::<String>::new());
    let connection_error = RwSignal::new(Option::<String>::None);

    // Cancel flag: flipped on component unmount. The spawned async task checks
    // this each iteration and breaks out cleanly, closing the EventSource.
    let cancelled = RwSignal::new(false);
    on_cleanup(move || {
        cancelled.set(true);
    });

    let backend_for_effect = backend_name.clone();
    Effect::new(move |_| {
        let backend = backend_for_effect.clone();
        if backend.is_empty() {
            return;
        }

        spawn_local(async move {
            // Exponential backoff for reconnection attempts.
            const INITIAL_DELAY_MS: u32 = 1_000;
            const MAX_DELAY_MS: u32 = 30_000;

            let mut delay_ms: u32 = INITIAL_DELAY_MS;
            let mut is_reconnecting = false;

            loop {
                if cancelled.get_untracked() {
                    break;
                }

                // If we're reconnecting, show the reconnecting message and wait.
                if is_reconnecting {
                    connection_error.set(Some("Connection lost — retrying...".to_string()));
                    gloo_timers::future::TimeoutFuture::new(delay_ms).await;
                    delay_ms = (delay_ms * 2).min(MAX_DELAY_MS);
                }

                let url = format!("/tama/v1/logs/{}/events", backend);
                let mut es = match EventSource::new(&url) {
                    Ok(es) => es,
                    Err(e) => {
                        connection_error.set(Some(format!("Failed to open SSE stream: {e:?}")));
                        is_reconnecting = true;
                        continue;
                    }
                };

                let mut log_stream = match es.subscribe("log") {
                    Ok(s) => s,
                    Err(e) => {
                        connection_error.set(Some(format!(
                            "Failed to subscribe to log events: {e:?}"
                        )));
                        es.close();
                        is_reconnecting = true;
                        continue;
                    }
                };

                // Reset reconnecting state on successful connection.
                is_reconnecting = false;
                connection_error.set(None);
                delay_ms = INITIAL_DELAY_MS;

                while let Some(Ok((_, msg))) = log_stream.next().await {
                    if cancelled.get_untracked() {
                        es.close();
                        return;
                    }

                    let data = msg.data().as_string().unwrap_or_default();
                    if let Ok(payload) = serde_json::from_str::<LogPayload>(&data) {
                        lines.update(|v| {
                            v.push(payload.line);
                            if v.len() > 1000 {
                                let drop_count = v.len() - 1000;
                                v.drain(0..drop_count);
                            }
                        });
                    }
                }

                // Stream ended — attempt reconnection.
                es.close();
                is_reconnecting = true;
            }
        });
    });

    let on_close_handler = move |_| {
        if let Some(cb) = &on_close {
            cb.run(());
        }
    };

    view! {
        <div
            style="border:1px solid var(--border,#ccc);border-radius:6px;background:#0f172a;color:#e2e8f0;font-family:monospace;font-size:0.75rem;max-height:400px;display:flex;flex-direction:column;"
        >
            // Header bar
            <div style="display:flex;justify-content:space-between;align-items:center;padding:0.5rem 0.75rem;background:#1e293b;border-bottom:1px solid #334155;">
                <div style="font-weight:600;font-size:0.875rem;">"📋 " {move || backend_name.clone()} " logs"</div>
                <button
                    type="button"
                    style="background:none;border:none;color:#94a3b8;cursor:pointer;font-size:1.25rem;line-height:1;"
                    on:click=on_close_handler
                    title="Close"
                >
                    "×"
                </button>
            </div>

            // Content area
            <div style="overflow-y:auto;padding:0.5rem 0.75rem;flex:1;">
                {move || {
                    if let Some(err) = connection_error.get() {
                        view! {
                            <div class="log-line log-line--error">{err}</div>
                        }.into_any()
                    } else {
                        let all_lines = lines.get();
                        if all_lines.is_empty() {
                            view! {
                                <div style="color:#94a3b8;">"Connecting..."</div>
                            }.into_any()
                        } else {
                            view! {
                                <pre style="margin:0;white-space:pre-wrap;word-break:break-all;line-height:1.4;">
                                    {all_lines.into_iter().map(|line| {
                                        let cls = format!("log-line {}", log_level_class(&line));
                                        view! { <div class=cls>{line}</div> }
                                    }).collect::<Vec<_>>()}
                                </pre>
                            }.into_any()
                        }
                    }
                }}
            </div>
        </div>
    }
}
