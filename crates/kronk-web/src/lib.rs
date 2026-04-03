use leptos::prelude::*;

#[component]
pub fn App() -> impl IntoView {
    view! { <h1>"Kronk Control Plane"</h1> }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    // In Leptos 0.7, mount_to_body takes a FnOnce closure, NOT a component fn directly.
    leptos::mount::mount_to_body(|| view! { <App /> });
}
