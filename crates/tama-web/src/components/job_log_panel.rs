//! Job log panel - displays live build logs via Server-Sent Events.

#[cfg(not(feature = "ssr"))]
mod wasm_impl {
    use crate::components::activity_panel::ActivityPanel;
    use crate::utils::sse_stream;
    use futures_util::StreamExt;
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

    #[component]
    pub fn JobLogPanel(
        job_id: String,
        #[prop(optional)] on_close: Option<Callback<()>>,
        #[prop(optional)] on_result: Option<Callback<String>>,
        #[prop(optional)] on_status: Option<Callback<String>>,
    ) -> impl IntoView {
        let lines = RwSignal::new(Vec::<String>::new());
        let status = RwSignal::new(String::from("running"));
        let cancelled = RwSignal::new(false);

        on_cleanup(move || {
            cancelled.set(true);
        });

        let connection_error = RwSignal::new(Option::<String>::None);
        let connection_error_for_async = connection_error;

        let job_id_for_effect = job_id.clone();
        Effect::new(move |_| {
            let job_id = job_id_for_effect.clone();
            if job_id.is_empty() {
                return;
            }

            let url = format!("/tama/v1/backends/jobs/{job_id}/events");
            let conn = sse_stream::create(url, cancelled, None);

            wasm_bindgen_futures::spawn_local(async move {
                loop {
                    if cancelled.get_untracked() {
                        break;
                    }

                    // Connect (or reconnect) with exponential backoff
                    match conn.connect_once().await {
                        Ok(()) => {}
                        Err(e) => {
                            connection_error_for_async.set(Some(e.to_string()));
                            // For infinite retry (None config), connect_once only
                            // returns Err on cancellation or max_attempts.
                            // Since we use None (infinite), this only happens on cancel.
                            break;
                        }
                    }

                    // Reset error on successful connection
                    connection_error_for_async.set(None);

                    // Subscribe to channels
                    let mut log_stream = match conn.subscribe("log") {
                        Ok(s) => s,
                        Err(_) => {
                            continue;
                        } // connection dropped, loop back
                    };
                    let mut status_stream = match conn.subscribe("status") {
                        Ok(s) => s,
                        Err(_) => {
                            continue;
                        }
                    };
                    let mut result_stream = match conn.subscribe("result") {
                        Ok(s) => s,
                        Err(_) => {
                            continue;
                        }
                    };

                    // Inner event processing loop - same select pattern as current code
                    let mut done = false;
                    loop {
                        if cancelled.get_untracked() {
                            break;
                        }

                        let next_log = log_stream.next();
                        let next_status = status_stream.next();
                        let next_result = result_stream.next();
                        futures_util::pin_mut!(next_log, next_status, next_result);
                        let first = futures_util::future::select(next_log, next_status);
                        match futures_util::future::select(first, next_result).await {
                            futures_util::future::Either::Left((inner, _)) => {
                                match inner {
                                    futures_util::future::Either::Left((Some(Ok(event)), _)) => {
                                        // Log event
                                        if let Ok(payload) =
                                            serde_json::from_str::<LogPayload>(&event.data)
                                        {
                                            lines.update(|v| {
                                                v.push(payload.line);
                                                if v.len() > 1000 {
                                                    v.drain(0..v.len() - 1000);
                                                }
                                            });
                                        }
                                    }
                                    futures_util::future::Either::Right((Some(Ok(event)), _)) => {
                                        // Status event
                                        if let Ok(payload) =
                                            serde_json::from_str::<StatusPayload>(&event.data)
                                        {
                                            status.set(payload.status.clone());
                                            if let Some(cb) = on_status.as_ref() {
                                                cb.run(payload.status.clone());
                                            }
                                            if payload.status != "running" {
                                                done = true;
                                                break; // terminal status — exit inner loop
                                            }
                                        }
                                    }
                                    _ => {
                                        break;
                                    } // stream ended, loop back for reconnect
                                }
                            }
                            futures_util::future::Either::Right((Some(Ok(event)), _)) => {
                                // Result event
                                if let Ok(payload) =
                                    serde_json::from_str::<ResultPayload>(&event.data)
                                {
                                    if let Some(cb) = on_result.as_ref() {
                                        cb.run(payload.results);
                                    }
                                }
                            }
                            _ => {
                                break;
                            } // stream ended
                        }
                    }
                    // If job reached terminal status, stop reconnecting.
                    // Otherwise (stream ended unexpectedly), outer loop reconnects.
                    if done {
                        break;
                    }
                }
            });
        });

        view! {
            <ActivityPanel
                title="Build logs".to_string()
                status=status
                connection_error=connection_error
                on_close=on_close
            >
                {move || {
                    let all_lines = lines.get();
                    if all_lines.is_empty() {
                        view! {
                            <div class="activity-panel__connecting">"Connecting..."</div>
                        }.into_any()
                    } else {
                        view! {
                            <pre class="activity-panel__logs">
                                {all_lines.join("\n")}
                            </pre>
                        }.into_any()
                    }
                }}
            </ActivityPanel>
        }
    }
}

#[cfg(feature = "ssr")]
mod ssr_impl {
    use leptos::prelude::*;

    /// SSR stub - no-op component for server-side rendering.
    #[component]
    pub fn JobLogPanel(
        #[allow(unused_variables)] job_id: String,
        #[prop(optional)]
        #[allow(unused_variables)]
        on_close: Option<Callback<()>>,
        #[prop(optional)]
        #[allow(unused_variables)]
        on_result: Option<Callback<String>>,
        #[prop(optional)]
        #[allow(unused_variables)]
        on_status: Option<Callback<String>>,
    ) -> impl IntoView {
        view! { <div></div> }.into_any()
    }
}

#[cfg(feature = "ssr")]
pub use ssr_impl::JobLogPanel;
#[cfg(not(feature = "ssr"))]
pub use wasm_impl::JobLogPanel;
