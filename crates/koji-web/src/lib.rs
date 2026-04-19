#[cfg(feature = "ssr")]
pub mod server;

#[cfg(feature = "ssr")]
pub mod api;

#[cfg(feature = "ssr")]
pub mod jobs;

#[cfg(feature = "ssr")]
pub mod types;

use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
mod components;
pub mod constants;
mod pages;
pub mod utils;

use crate::components::toast::{DownloadEvent, ToastStore};

/// Debounce flag for progress event processing — prevents spawning too many async tasks.
static PROGRESS_DEBOUNCE: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(false));

#[component]
pub fn App() -> impl IntoView {
    let toast_store = ToastStore::global();

    // Open SSE connection on app mount to receive download events
    let es =
        web_sys::EventSource::new("/api/downloads/events").expect("Failed to create EventSource");

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
                    // Debounce progress events to avoid spawning too many async tasks.
                    if matches!(
                        event_json.event.as_str(),
                        "Started" | "Progress" | "Verifying"
                    ) {
                        let is_debounced = PROGRESS_DEBOUNCE.swap(true, Ordering::SeqCst);
                        // Skip this progress update if one is already in flight.
                        if event_json.event.as_str() == "Progress" && is_debounced {
                            // Reset debounce flag after a short delay so the next batch
                            // of progress events can be processed.
                            wasm_bindgen_futures::spawn_local(async move {
                                gloo_timers::future::TimeoutFuture::new(200).await;
                                PROGRESS_DEBOUNCE.store(false, Ordering::SeqCst);
                            });
                        } else {
                            let job_id = event_json.job_id.clone();
                            let status_label = match event_json.event.as_str() {
                                "Started" => "running",
                                "Progress" => "running",
                                "Verifying" => "verifying",
                                _ => "running",
                            };
                            let bytes_down = event_json.bytes_downloaded;
                            let total_bytes = event_json.total_bytes;
                            pages::downloads::ACTIVE_DOWNLOADS.update(|items| {
                                if let Some(item) = items.iter_mut().find(|i| i.job_id == job_id) {
                                    item.status = status_label.to_string();
                                    if let Some(bytes) = bytes_down {
                                        item.bytes_downloaded = bytes as i64;
                                    }
                                    if let Some(total) = total_bytes {
                                        item.total_bytes = Some(total as i64);
                                    }
                                    // Recalculate progress
                                    if let Some(total) = item.total_bytes {
                                        if total > 0 {
                                            item.progress_percent =
                                                (item.bytes_downloaded as f64 / total as f64) * 100.0;
                                        }
                                    }
                                }
                            });
                            // Reset debounce flag after a short delay so the next batch
                            // of progress events can be processed.
                            wasm_bindgen_futures::spawn_local(async move {
                                gloo_timers::future::TimeoutFuture::new(200).await;
                                PROGRESS_DEBOUNCE.store(false, Ordering::SeqCst);
                            });
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

                    // Refresh history on terminal events (Completed/Failed/Cancelled)
                    // Always refresh page 0 so new items are visible immediately.
                    if matches!(
                        event_json.event.as_str(),
                        "Completed" | "Failed" | "Cancelled"
                    ) {
                        let limit = pages::downloads::HISTORY_LIMIT.get();
                        wasm_bindgen_futures::spawn_local(async move {
                            if let Ok(resp) = gloo_net::http::Request::get(&format!(
                                "/api/downloads/history?limit={}&offset=0",
                                limit
                            ))
                            .send()
                            .await
                            {
                                if let Ok(data) =
                                    resp.json::<pages::downloads::DownloadsHistoryResponse>()
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

    view! {
        <Router>
            <components::sidebar::Sidebar />
            <main>
                <Routes fallback=|| "Page not found">
                    <Route path=path!("/") view=pages::dashboard::Dashboard />
                    <Route path=path!("/models") view=pages::models::Models />
                    <Route path=path!("/models/:id/edit") view=pages::model_editor::ModelEditor />
                    <Route path=path!("/backends") view=pages::backends::Backends />
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
