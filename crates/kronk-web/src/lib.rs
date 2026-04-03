#[cfg(feature = "ssr")]
pub mod server;

#[cfg(feature = "ssr")]
mod api;

use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};
mod components;
mod pages;

#[component]
pub fn App() -> impl IntoView {
    view! {
        <Router>
            <components::nav::Nav />
            <main>
                <Routes fallback=|| "Page not found">
                    <Route path=path!("/") view=pages::dashboard::Dashboard />
                    <Route path=path!("/models") view=pages::models::Models />
                    <Route path=path!("/pull") view=pages::pull::Pull />
                    <Route path=path!("/logs") view=pages::logs::Logs />
                    <Route path=path!("/config") view=pages::config_editor::ConfigEditor />
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
