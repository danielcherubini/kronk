use crate::components::pull_wizard::*;
use std::collections::{HashMap, HashSet};

/// Dropdown + conditional custom input for selecting context length for a single file.
///
/// Always renders a <select> dropdown. When the user picks "Custom...", the
/// `is_custom` signal flips to true and a number input appears below it.
/// We use CSS visibility instead of conditional view rendering to avoid
/// Leptos closure-ownership issues with String props.
#[component]
fn ContextFileDropdown(
    filename: String,
    context_lengths: RwSignal<HashMap<String, u32>>,
    is_custom: RwSignal<bool>,
) -> impl IntoView {
    let filename_dropdown = filename.clone();
    let filename_onchange = filename.clone();
    let filename_input = filename.clone();
    let filename_oninput = filename.clone();
    let filename_options = filename.clone();

    view! {
        <div>
            // Always-visible dropdown
            <select
                class="form-select input-narrow"
                prop:value=move || {
                    if is_custom.get() {
                        "custom".to_string()
                    } else {
                        context_lengths.get()
                            .get(&filename_dropdown)
                            .copied()
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "32768".to_string())
                    }
                }
                on:change=move |e| {
                    let v = event_target_value(&e);
                    if v == "custom" {
                        is_custom.set(true);
                    } else if let Ok(parsed) = v.parse::<u32>() {
                        context_lengths.update(|m| {
                            m.insert(filename_onchange.clone(), parsed);
                        });
                        is_custom.set(false);
                    }
                }
            >
                {move || {
                    let current = context_lengths.get().get(&filename_options).copied().unwrap_or(32768);
                    let is_c = is_custom.get();
                    CONTEXT_VALUES.iter().map(|v| {
                        let val = *v;
                        let selected = !is_c && val == current;
                        view! {
                            <option value=val.to_string() selected=selected>{*v}</option>
                        }
                    }).collect::<Vec<_>>()
                }}
                <option value="custom">"Custom..."</option>
            </select>

            // Conditional custom input - rendered always, hidden via CSS
            <input
                class="form-input input-narrow"
                type="number"
                min="512"
                step="512"
                style=move || if is_custom.get() { "display:block;margin-top:0.5rem" } else { "display:none" }
                prop:value=move || {
                    context_lengths.get()
                        .get(&filename_input)
                        .copied()
                        .unwrap_or(32768)
                }
                on:input=move |e| {
                    if let Ok(v) = event_target_value(&e).parse::<u32>() {
                        context_lengths.update(|m| {
                            m.insert(filename_oninput.clone(), v);
                        });
                    }
                }
            />
        </div>
    }
}

#[component]
pub fn ContextStep(
    selected_filenames: Signal<HashSet<String>>,
    available_quants: Signal<Vec<QuantEntry>>,
    context_lengths: RwSignal<HashMap<String, u32>>,
    on_next: Callback<()>,
    on_back: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Set Context Length"</h2>
            <p class="form-card__desc text-muted">
                "Configure the context window size for each selected quantisation."
            </p>
        </div>

        <table class="data-table">
            <thead>
                <tr>
                    <th>"Quant"</th>
                    <th>"Filename"</th>
                    <th>"Context Length"</th>
                </tr>
            </thead>
            <tbody>
                {move || {
                    let sel = selected_filenames.get();
                    available_quants.get().into_iter()
                        .filter(|q| sel.contains(&q.filename))
                        .map(|q| {
                            let fname = q.filename.clone();
                            let label = q.quant.clone().unwrap_or_else(|| fname.clone());
                            let is_custom = RwSignal::new(false);

                            view! {
                                <tr>
                                    <td><span class="badge badge-info">{label}</span></td>
                                    <td><code>{q.filename.clone()}</code></td>
                                    <td>
                                        <ContextFileDropdown
                                            filename=fname
                                            context_lengths
                                            is_custom
                                        />
                                    </td>
                                </tr>
                            }
                        }).collect::<Vec<_>>()
                }}
            </tbody>
        </table>

        <div class="form-actions mt-3">
            <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                "Back"
            </button>
            <button class="btn btn-primary" on:click=move |_| on_next.run(())>
                "Next →"
            </button>
        </div>
    }
}
