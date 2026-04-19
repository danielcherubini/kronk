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
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
mod components;
pub mod constants;
mod pages;
pub mod utils;

use crate::components::toast::{DownloadEvent, ToastStore};

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
                    if matches!(
                        event_json.event.as_str(),
                        "Started" | "Progress" | "Verifying" | "Queued"
                    ) {
                        wasm_bindgen_futures::spawn_local(async move {
                            if let Ok(resp) = gloo_net::http::Request::get("/api/downloads/active")
                                .send()
                                .await
                            {
                                if let Ok(data) = resp
                                    .json::<pages::downloads::DownloadsActiveResponse>()
                                    .await
                                {
                                    pages::downloads::ACTIVE_DOWNLOADS.set(data.items);
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
