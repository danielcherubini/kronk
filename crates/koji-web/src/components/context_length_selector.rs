use leptos::prelude::*;
use wasm_bindgen::JsCast;
use crate::constants::CONTEXT_VALUES;

#[component]
pub fn ContextLengthSelector(
    /// The current context length. `None` indicates "use default".
    value: Signal<Option<u32>>,
    /// Callback to notify the parent of a value change.
    on_change: Callback<Option<u32>>,
    /// A key (e.g. Model ID) that triggers an internal state reset when changed.
    #[prop(into)]
    reset_key: Signal<String>,
    /// Optional CSS classes for layout.
    #[prop(into, optional)]
    class: Option<String>,
) -> impl IntoView {
    let is_custom = RwSignal::new(false);

    // Sync internal state with external value:
    // If the value changes to something that is a known preset, reset is_custom to false.
    // If the value is a custom number, set is_custom to true.
    Effect::new(move |_| {
        let val = value.get();
        is_custom.set(val.filter(|v| !CONTEXT_VALUES.contains(v)).is_some());
    });

    // Reset internal state when the reset_key changes (e.g. switching models)
    Effect::new(move |_| {
        let _ = reset_key.get();
        is_custom.set(false);
    });

    // Reference to the numeric input for imperative updates
    let input_ref = StoredValue::new(None::<web_sys::HtmlInputElement>);

    // Imperatively set the numeric input value to avoid the Leptos prop:value cursor-jump bug.
    Effect::new(move |_| {
        let val = value.get();
        let custom = is_custom.get();
        if custom {
            if let Some(el) = input_ref.get_value() {
                let display_val = val.map(|v| v.to_string()).unwrap_or_default();
                if el.value() != display_val {
                    el.set_value(&display_val);
                }
            }
        }
    });

    view! {
        <div class=move || class.clone().unwrap_or_default()>
            <select
                class="form-select"
                prop:value=move || {
                    if is_custom.get() {
                        "custom".to_string()
                    } else if value.get().is_none() {
                        "".to_string()
                    } else {
                        value.get().unwrap_or(0).to_string()
                    }
                }
                on:change=move |e| {
                    let v = crate::utils::target_value(&e);
                    if v == "custom" {
                        is_custom.set(true);
                    } else if v.is_empty() {
                        is_custom.set(false);
                        on_change.run(None);
                    } else if let Ok(parsed) = v.parse::<u32>() {
                        is_custom.set(false);
                        on_change.run(Some(parsed));
                    }
                }
            >
                <option value="">"Default"</option>
                {CONTEXT_VALUES.iter().map(|v| {
                    let val_str = v.to_string();
                    view! { <option value=val_str.clone()>{val_str.clone()}</option> }
                }).collect::<Vec<_>>()}
                <option value="custom">"Custom..."</option>
            </select>

            <input
                class="form-input"
                type="number"
                min="512"
                step="512"
                style=move || if is_custom.get() { "display:block;margin-top:0.5rem" } else { "display:none" }
                on:input=move |e| {
                    let val = crate::utils::target_value(&e);
                    if let Ok(parsed) = val.parse::<u32>() {
                        on_change.run(Some(parsed));
                    } else if val.is_empty() {
                        on_change.run(None);
                    }
                }
                // Use on:mount to capture the element reference
                on:mount=move |el: web_sys::Element| {
                    input_ref.set_value(Some(el.dyn_into::<web_sys::HtmlInputElement>().unwrap()));
                }
            />
        </div>
    }
}
