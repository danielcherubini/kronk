//! Job Log Panel Component
//!
//! Displays live build logs via EventSource SSE.

#[cfg(feature = "ssr")]
use leptos::ev::MouseEvent;
#[cfg(feature = "ssr")]
use leptos::prelude::*;

#[cfg(feature = "ssr")]
/// JobLogPanel displays live build logs via SSE
///
/// # Arguments
/// * `job_id` - The ID of the job to display logs for
/// * `on_close` - Callback when user closes the panel
#[component]
pub fn JobLogPanel(
    #[prop(into)] job_id: Signal<String>,
    #[prop(into)] on_close: EventHandler<MouseEvent>,
) -> impl IntoView {
    let (logs, set_logs) = create_signal::<Vec<String>>(vec![]);
    let (is_connected, set_is_connected) = create_signal(false);
    let (has_error, set_has_error) = create_signal(false);
    let (last_status, set_last_status) = create_signal::<Option<String>>(None);

    // Connect to SSE stream when job_id changes
    let job_id_for_stream = job_id;
    let set_logs_for_stream = set_logs;
    let set_is_connected_for_stream = set_is_connected;
    let set_has_error_for_stream = set_has_error;
    let set_last_status_for_stream = set_last_status;

    create_effect(move |_| {
        let current_job_id = job_id_for_stream();
        if current_job_id.is_empty() {
            set_is_connected_for_stream.set(false);
            set_has_error_for_stream.set(false);
            set_last_status_for_stream.set(None);
            return;
        }

        // Clear logs when job changes
        set_logs_for_stream.set(vec![]);
        set_is_connected_for_stream.set(true);
        set_has_error_for_stream.set(false);
        set_last_status_for_stream.set(None);

        // Connect to SSE endpoint
        let event_source_url = format!("/api/backends/jobs/{}/events", current_job_id);

        // Use JavaScript fetch for SSE (browser-only)
        let _ = web_sys::window().map(|window| {
            let js_code = format!(
                "(async function() {{\n\
                 try {{\n\
                 const response = await fetch('{}');\n\
                 if (!response.ok) throw new Error('Failed to connect to logs');\n\
                 const reader = response.body.getReader();\n\
                 const decoder = new TextDecoder();\n\
                 let buffer = '';\n\
                 while (true) {{\n\
                 const {{ done, value }} = await reader.read();\n\
                 if (done) break;\n\
                 const text = decoder.decode(value, {{ stream: true }});\n\
                 buffer += text;\n\
                 const lines = buffer.split('\\n\\n');\n\
                 buffer = lines.pop() || '';\n\
                 for (const line of lines) {{\n\
                 if (line.startsWith('data: ')) {{\n\
                 const data = line.slice(6);\n\
                 try {{\n\
                 const parsed = JSON.parse(data);\n\
                 if (parsed.event === 'log') {{\n\
                 window.__koji_log_lines__ = window.__koji_log_lines__ || [];\n\
                 window.__koji_log_lines__.push(parsed.data.line);\n\
                 window.dispatchEvent(new CustomEvent('koji-logs-update'));\n\
                 }} else if (parsed.event === 'status') {{\n\
                 window.__koji_last_status__ = parsed.data.status;\n\
                 window.dispatchEvent(new CustomEvent('koji-status-update'));\n\
                 }}\n\
                 }} catch (e) {{\n\
                 // Ignore parse errors\n\
                 }}\n\
                 }}\n\
                 }}\n\
                 }} catch (e) {{\n\
                 window.dispatchEvent(new CustomEvent('koji-logs-error', {{ detail: e.message }}));\n\
                 }}\n\
                 }})();",
                event_source_url
            );
            let _ = window.eval(&js_code);
        });

        // Listen for log updates
        let set_logs_clone = set_logs_for_stream.clone();
        let set_error_clone = set_has_error_for_stream.clone();

        if let Some(window) = web_sys::window() {
            let handler = Closure::wrap(Box::new(move |_: web_sys::Event| {
                if let Some(window) = web_sys::window() {
                    if let Ok(logs_js) = window.get_field("__koji_log_lines__") {
                        if let Ok(logs_vec) = serde_wasm_bindgen::from_value(logs_js) {
                            set_logs_clone.set(logs_vec);
                        }
                    }
                }
            }) as Box<dyn FnMut(_)>);

            window
                .add_event_listener_with_callback(
                    "koji-logs-update",
                    handler.as_ref().unchecked_ref(),
                )
                .ok();
            handler.forget();

            let error_handler = Closure::wrap(Box::new(move |e: web_sys::Event| {
                if let Some(event) = e.dyn_ref::<web_sys::CustomEvent>() {
                    if let Some(msg) = event.data().as_string() {
                        set_error_clone.set(true);
                        set_logs_clone.set(vec![format!("Error: {}", msg)]);
                    }
                }
            }) as Box<dyn FnMut(_)>);

            window
                .add_event_listener_with_callback(
                    "koji-logs-error",
                    error_handler.as_ref().unchecked_ref(),
                )
                .ok();
            error_handler.forget();

            // Listen for status updates
            let set_status_clone = set_last_status_for_stream.clone();
            let status_handler = Closure::wrap(Box::new(move |_: web_sys::Event| {
                if let Some(window) = web_sys::window() {
                    if let Ok(status_js) = window.get_field("__koji_last_status__") {
                        if let Ok(status_str) = serde_wasm_bindgen::from_value(status_js) {
                            set_status_clone.set(Some(status_str));
                        }
                    }
                }
            }) as Box<dyn FnMut(_)>);

            window
                .add_event_listener_with_callback(
                    "koji-status-update",
                    status_handler.as_ref().unchecked_ref(),
                )
                .ok();
            status_handler.forget();
        }
    });

    // Auto-close when job completes
    create_effect(move |_| {
        if let Some(status) = last_status() {
            if status != "running" && status != "Running" {
                // Job completed, auto-close after a short delay
                spawn_local(async move {
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    // Note: We can't call on_close from here directly due to closure lifetime
                    // This is a limitation - in practice, user should close manually
                });
            }
        }
    });

    view! {
        <div class="job-log-panel">
            <div class="job-log-header">
                <span class="job-log-title">Build Logs</span>
                <button
                    class="job-log-close"
                    on:click=on_close
                >
                    X
                </button>
            </div>

            <div class="job-log-content">
                {
                    move || {
                        if !is_connected() {
                            view! { <div class="job-log-status">Connecting...</div> }.into_view()
                        } else if has_error() {
                            view! { <div class="job-log-status error">Connection lost</div> }.into_view()
                        } else {
                            let all_logs = logs.get();
                            view! {
                                <div class="job-log-logs">
                                    {
                                        all_logs.into_iter().map(|log| {
                                            view! {
                                                <div class="job-log-line">{log}</div>
                                            }
                                            .into_view()
                                        }).collect::<Vec<_>>()
                                    }
                                </div>
                            }.into_view()
                        }
                    }
                }
            </div>
        </div>
    }.into_view()
}
