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
mod components;
pub mod constants;
mod pages;
pub mod utils;

#[component]
pub fn App() -> impl IntoView {
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
                </Routes>
            </main>
        </Router>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    // In Leptos 0.7, mount_to_body takes a FnOnce closure, NOT a component fn directly.
    leptos::mount::mount_to_body(|| view! { <App /> });
}
