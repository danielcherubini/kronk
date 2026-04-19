pub mod self_update;

/// Format bytes into a human-readable string (KB, MB, GB).
pub fn format_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.1} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{} B", bytes)
    }
}

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
