#![allow(dead_code)]

#[cfg(feature = "ssr")]
pub mod server;

#[cfg(feature = "ssr")]
pub mod api;

#[cfg(feature = "ssr")]
pub mod jobs;

#[cfg(feature = "ssr")]
pub mod gpu;

#[cfg(feature = "ssr")]
pub mod types;

use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};
use std::sync::atomic::{AtomicU64, Ordering};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

/// Log an error to the browser console (WASM-safe, no tracing dependency).
fn log_error(msg: &str) {
    web_sys::console::error_1(&JsValue::from_str(msg));
}

/// Log an info message to the browser console (WASM-safe).
fn log_info(msg: &str) {
    web_sys::console::log_1(&JsValue::from_str(msg));
}

/// Log a warning to the browser console (WASM-safe).
fn log_warn(msg: &str) {
    web_sys::console::warn_1(&JsValue::from_str(msg));
}

mod components;
pub mod constants;
mod pages;
pub mod utils;

use crate::components::toast::{DownloadEvent, ToastStore};

/// Timestamp (in ms) of the last processed progress event.
/// Used to throttle SSE progress updates — only process events that are
/// at least 200ms apart, preventing dozens of async tasks from being spawned.
static LAST_PROGRESS_TS: AtomicU64 = AtomicU64::new(0);

/// Get current timestamp in milliseconds.
pub fn now_ms() -> u64 {
    web_sys::js_sys::Date::now() as u64
}

/// Check if enough time has passed since the last progress event to process a new one.
/// Returns true if this event should be processed, false if it should be throttled.
fn should_process_progress_event() -> bool {
    let now = now_ms();
    let last_ts = LAST_PROGRESS_TS.load(Ordering::Relaxed);
    if now - last_ts >= 200 {
        // Update the timestamp and allow this event through
        LAST_PROGRESS_TS.store(now, Ordering::Relaxed);
        true
    } else {
        false
    }
}

#[component]
pub fn App() -> impl IntoView {
    let toast_store = ToastStore::global();

    // Track SSE connectivity state for offline indicator.
    // `true` = connected, `false` = offline/error.
    let sse_connected = RwSignal::new(true);

    // Open SSE connection on app mount to receive download events.
    // Handle creation failure gracefully — show offline indicator and retry periodically.
    let es_result = web_sys::EventSource::new("/api/downloads/events");
    let es: Option<web_sys::EventSource> = match es_result {
        Ok(es) => Some(es),
        Err(err) => {
            sse_connected.set(false);
            let err_msg = err
                .as_string()
                .unwrap_or_else(|| "unknown error".to_string());
            log_error(&format!(
                "Failed to create EventSource for download events: {err_msg}. Showing offline indicator."
            ));
            // Retry periodically in the background every 5 seconds.
            let retry_url = "/api/downloads/events".to_string();
            let toast_store_for_retry = toast_store.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut attempt = 0u32;
                loop {
                    gloo_timers::future::TimeoutFuture::new(5_000).await;
                    attempt += 1;
                    match web_sys::EventSource::new(&retry_url) {
                        Ok(new_es) => {
                            log_info(&format!(
                                "EventSource reconnected successfully after {attempt} attempts"
                            ));
                            sse_connected.set(true);

                            // Attach event listeners to the newly created EventSource.
                            for event_name in [
                                "Started",
                                "Progress",
                                "Verifying",
                                "Completed",
                                "Failed",
                                "Cancelled",
                                "Queued",
                            ] {
                                let toast_store = toast_store_for_retry.clone();
                                let handler =
                                    Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
                                        if let Some(data) = event.data().as_string() {
                                            if let Ok(event_json) =
                                                serde_json::from_str::<DownloadEvent>(&data)
                                            {
                                                if let Some(toast) =
                                                    ToastStore::from_download_event(&event_json)
                                                {
                                                    toast_store.add(toast);
                                                }
                                            }
                                        }
                                    })
                                        as Box<dyn FnMut(_)>);
                                new_es
                                    .add_event_listener_with_callback(
                                        event_name,
                                        handler.as_ref().unchecked_ref(),
                                    )
                                    .unwrap();
                                handler.forget();
                            }

                            break;
                        }
                        Err(e) => {
                            let e_msg =
                                e.as_string().unwrap_or_else(|| "unknown error".to_string());
                            log_warn(&format!(
                                "EventSource reconnect attempt {attempt} failed: {e_msg}"
                            ));
                        }
                    }
                }
            });
            // EventSource creation failed — None means no listeners will be attached.
            // The offline indicator banner is already shown; the app continues normally.
            None
        }
    };

    if let Some(es) = es {
        for event_name in [
            "Started",
            "Progress",
            "Verifying",
            "Completed",
            "Failed",
            "Cancelled",
            "Queued",
        ] {
            let toast_store = toast_store.clone();
            let handler = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
                if let Some(data) = event.data().as_string() {
                    if let Ok(event_json) = serde_json::from_str::<DownloadEvent>(&data) {
                        // Update active downloads from progress/started events
                        // by updating the specific item in-place instead of a full refresh.
                        // Throttle Progress events to 1 per 200ms to avoid spawning too many async tasks.
                        if matches!(
                            event_json.event.as_str(),
                            "Started" | "Progress" | "Verifying"
                        ) {
                            // Only throttle 'Progress' events — Started/Verifying are infrequent
                            // and should always be processed immediately.
                            let should_process = if event_json.event.as_str() == "Progress" {
                                should_process_progress_event()
                            } else {
                                true
                            };

                            if should_process {
                                let job_id = event_json.job_id.clone();
                                let status_label = match event_json.event.as_str() {
                                    "Started" => "running",
                                    "Progress" => "running",
                                    "Verifying" => "verifying",
                                    _ => "running",
                                };
                                let bytes_down = event_json.bytes_downloaded;
                                let total_bytes = event_json.total_bytes;
                                // Use .set() with a new Vec so ArcRwSignal detects the change.
                                // .update() mutates in-place but doesn't trigger re-renders.
                                let current = pages::downloads::ACTIVE_DOWNLOADS.get_untracked();
                                let updated: Vec<_> = current
                                    .into_iter()
                                    .map(|mut item| {
                                        if item.job_id == job_id {
                                            item.status = status_label.to_string();
                                            if let Some(bytes) = bytes_down {
                                                item.bytes_downloaded = bytes as i64;
                                            }
                                            if let Some(total) = total_bytes {
                                                item.total_bytes = Some(total as i64);
                                            }
                                        }
                                        item
                                    })
                                    .collect();
                                pages::downloads::ACTIVE_DOWNLOADS.set(updated);
                            }
                        }

                        // For Queued events, fetch full details to get missing fields
                        if event_json.event.as_str() == "Queued" {
                            let job_id = event_json.job_id.clone();
                            wasm_bindgen_futures::spawn_local(async move {
                                if let Ok(resp) =
                                    gloo_net::http::Request::get("/api/downloads/active")
                                        .send()
                                        .await
                                {
                                    if let Ok(data) = resp
                                        .json::<pages::downloads::DownloadsActiveResponse>()
                                        .await
                                    {
                                        // Only replace if this job isn't already in the list
                                        pages::downloads::ACTIVE_DOWNLOADS.update(|items| {
                                            if !items.iter().any(|i| i.job_id == job_id) {
                                                pages::downloads::ACTIVE_DOWNLOADS.set(data.items);
                                            }
                                        });
                                    }
                                }
                            });
                        }

                        // Refresh history on terminal events (Completed/Failed/Cancelled).
                        // Use the user's current page so they aren't unexpectedly jumped.
                        if matches!(
                            event_json.event.as_str(),
                            "Completed" | "Failed" | "Cancelled"
                        ) {
                            let limit = pages::downloads::HISTORY_LIMIT.get();
                            let offset = pages::downloads::HISTORY_PAGE.get()
                                * pages::downloads::HISTORY_LIMIT.get();
                            wasm_bindgen_futures::spawn_local(async move {
                                if let Ok(resp) = gloo_net::http::Request::get(&format!(
                                    "/api/downloads/history?limit={}&offset={}",
                                    limit, offset
                                ))
                                .send()
                                .await
                                {
                                    if let Ok(data) = resp
                                        .json::<pages::downloads::DownloadsHistoryResponse>()
                                        .await
                                    {
                                        pages::downloads::HISTORY_ITEMS.set(data.items);
                                        pages::downloads::HISTORY_TOTAL.set(data.total);
                                    }
                                }
                            });
                        }

                        // Emit toasts for relevant events
                        if let Some(toast) = ToastStore::from_download_event(&event_json) {
                            toast_store.add(toast);
                        }
                    }
                }
            }) as Box<dyn FnMut(_)>);
            es.add_event_listener_with_callback(event_name, handler.as_ref().unchecked_ref())
                .unwrap();
            handler.forget();
        }
    }

    view! {
        <Router>
            <components::sidebar::Sidebar />
            <main>
                <Show when=move || !sse_connected.get()>
                    <div style="background-color: #fef3c7; color: #92400e; padding: 8px 16px; text-align: center; font-size: 14px;">
                        "⚠ Download events unavailable — checking connection..."
                    </div>
                </Show>
                <Routes fallback=|| "Page not found">
                    <Route path=path!("/") view=pages::dashboard::Dashboard />
                    <Route path=path!("/models") view=pages::models::Models />
                    <Route path=path!("/models/:id/edit") view=pages::model_editor::ModelEditor />
                    <Route path=path!("/backends") view=pages::backends::Backends />
                    <Route path=path!("/benchmarks") view=pages::benchmarks::Benchmarks />
                    <Route path=path!("/logs") view=pages::logs::Logs />
                    <Route path=path!("/config") view=pages::config_editor::ConfigEditor />
                    <Route path=path!("/updates") view=pages::updates::Updates />
                    <Route path=path!("/downloads") view=pages::downloads::Downloads />
                </Routes>
            </main>
            <components::toast::ToastContainer store=toast_store />
        </Router>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    // In Leptos 0.7, mount_to_body takes a FnOnce closure, NOT a component fn directly.
    leptos::mount::mount_to_body(|| view! { <App /> });
}
