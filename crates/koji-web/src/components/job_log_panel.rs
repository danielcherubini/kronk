//! Job log panel - displays live build logs via Server-Sent Events.

use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use leptos::prelude::*;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct LogPayload {
    line: String,
}

#[derive(Debug, Clone, Deserialize)]
struct StatusPayload {
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ResultPayload {
    results: String,
}

/// JobLogPanel - subscribes to /api/backends/jobs/:id/events and streams logs.
#[component]
pub fn JobLogPanel(
    /// The job id whose logs we should display
    job_id: String,
    /// Called when user clicks Close
    #[prop(optional)]
    on_close: Option<Callback<()>>,
    /// Called with the JSON string payload when a "result" SSE event arrives.
    #[prop(optional)]
    on_result: Option<Callback<String>>,
    /// Called with the status string on each "status" SSE event.
    #[prop(optional)]
    on_status: Option<Callback<String>>,
) -> impl IntoView {
    let lines = RwSignal::new(Vec::<String>::new());
    let status = RwSignal::new(String::from("running"));
    let connection_error = RwSignal::new(Option::<String>::None);

    // Cancel flag: flipped on component unmount. The spawned async task checks
    // this each iteration and breaks out cleanly, closing the EventSource.
    let cancelled = RwSignal::new(false);
    on_cleanup(move || {
        cancelled.set(true);
    });

    let job_id_for_effect = job_id.clone();
    Effect::new(move |_| {
        let job_id = job_id_for_effect.clone();
        if job_id.is_empty() {
            return;
        }

        wasm_bindgen_futures::spawn_local(async move {
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

                    // Update delay for next attempt (exponential backoff).
                    delay_ms = (delay_ms * 2).min(MAX_DELAY_MS);
                }

                let url = format!("/api/backends/jobs/{job_id}/events");
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
                        connection_error
                            .set(Some(format!("Failed to subscribe to log events: {e:?}")));
                        es.close();
                        is_reconnecting = true;
                        continue;
                    }
                };

                let mut status_stream = match es.subscribe("status") {
                    Ok(s) => s,
                    Err(e) => {
                        connection_error
                            .set(Some(format!("Failed to subscribe to status events: {e:?}")));
                        es.close();
                        is_reconnecting = true;
                        continue;
                    }
                };

                let mut result_stream = match es.subscribe("result") {
                    Ok(s) => s,
                    Err(e) => {
                        connection_error
                            .set(Some(format!("Failed to subscribe to result events: {e:?}")));
                        es.close();
                        is_reconnecting = true;
                        continue;
                    }
                };

                // Reset reconnecting state on successful connection.
                is_reconnecting = false;
                connection_error.set(None);
                delay_ms = INITIAL_DELAY_MS;

                loop {
                    if cancelled.get_untracked() {
                        es.close();
                        return;
                    }

                    let next_log = log_stream.next();
                    let next_status = status_stream.next();
                    let next_result = result_stream.next();
                    futures_util::pin_mut!(next_log, next_status, next_result);

                    let first = futures_util::future::select(next_log, next_status);
                    match futures_util::future::select(first, next_result).await {
                        futures_util::future::Either::Left((inner, _)) => match inner {
                            futures_util::future::Either::Left((Some(Ok((_, msg))), _)) => {
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
                            futures_util::future::Either::Right((Some(Ok((_, msg))), _)) => {
                                let data = msg.data().as_string().unwrap_or_default();
                                if let Ok(payload) = serde_json::from_str::<StatusPayload>(&data) {
                                    status.set(payload.status.clone());
                                    if let Some(cb) = on_status.as_ref() {
                                        cb.run(payload.status.clone());
                                    }
                                    if payload.status != "running" {
                                        es.close();
                                        return;
                                    }
                                }
                            }
                            _ => {
                                // Stream ended — attempt reconnection.
                                es.close();
                                break;
                            }
                        },
                        futures_util::future::Either::Right((Some(Ok((_, msg))), _)) => {
                            let data = msg.data().as_string().unwrap_or_default();
                            if let Ok(payload) = serde_json::from_str::<ResultPayload>(&data) {
                                if let Some(cb) = on_result.as_ref() {
                                    cb.run(payload.results);
                                }
                            }
                        }
                        _ => {
                            // Stream ended — attempt reconnection.
                            es.close();
                            is_reconnecting = true;
                            break;
                        }
                    }
                }
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
            style="margin-top:1rem;border:1px solid var(--border,#ccc);border-radius:6px;background:#0f172a;color:#e2e8f0;font-family:monospace;font-size:0.75rem;max-height:300px;display:flex;flex-direction:column;"
        >
            <div style="display:flex;justify-content:space-between;align-items:center;padding:0.5rem 0.75rem;background:#1e293b;border-bottom:1px solid #334155;">
                <div style="display:flex;align-items:center;gap:0.5rem;">
                    <span style="font-weight:600;">"Build logs"</span>
                    <span style="font-size:0.75rem;color:#94a3b8;">
                        {move || {
                            let s = status.get();
                            match s.as_str() {
                                "running" => "● Running",
                                "succeeded" => "✓ Succeeded",
                                "failed" => "✗ Failed",
                                _ => "● Unknown",
                            }
                        }}
                    </span>
                </div>
                <button
                    type="button"
                    style="background:none;border:none;color:#94a3b8;cursor:pointer;font-size:1rem;"
                    on:click=on_close_handler
                >
                    "×"
                </button>
            </div>

            <div style="overflow-y:auto;padding:0.5rem 0.75rem;flex:1;">
                {move || {
                    if let Some(err) = connection_error.get() {
                        view! {
                            <div style="color:#ef4444;">{err}</div>
                        }.into_any()
                    } else {
                        let all_lines = lines.get();
                        if all_lines.is_empty() {
                            view! {
                                <div style="color:#94a3b8;">"Connecting..."</div>
                            }.into_any()
                        } else {
                            view! {
                                <pre style="margin:0;white-space:pre-wrap;word-break:break-all;">
                                    {all_lines.join("\n")}
                                </pre>
                            }.into_any()
                        }
                    }
                }}
            </div>
        </div>
    }
}
