pub mod self_update;

/// Helper to convert event.target.value as String
/// Handles input, select, and textarea elements uniformly
pub fn target_value(ev: &leptos::ev::Event) -> String {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| {
            t.dyn_into::<web_sys::HtmlInputElement>()
                .ok()
                .map(|i| i.value())
                .or_else(|| {
                    ev.target().and_then(|t| {
                        t.dyn_into::<web_sys::HtmlSelectElement>()
                            .ok()
                            .map(|s| s.value())
                    })
                })
                .or_else(|| {
                    ev.target().and_then(|t| {
                        t.dyn_into::<web_sys::HtmlTextAreaElement>()
                            .ok()
                            .map(|s| s.value())
                    })
                })
        })
        .unwrap_or_default()
}
