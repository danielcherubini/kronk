use std::collections::{HashMap, HashSet};

use crate::components::context_length_selector::ContextLengthSelector;
use crate::components::pull_wizard::*;

/// Dropdown + conditional custom input for selecting context length for a single file.
#[component]
fn ContextFileDropdown(
    filename: String,
    context_lengths: RwSignal<HashMap<String, u32>>,
) -> impl IntoView {
    let filename_val = filename.clone();
    let filename_change = filename.clone();

    view! {
        <ContextLengthSelector
            class="input-narrow".to_string()
            value=Signal::derive(move || context_lengths.get().get(&filename_val).copied())
            on_change=Callback::new(move |v: Option<u32>| {
                let val = v.unwrap_or(32768);
                context_lengths.update(|m| {
                    m.insert(filename_change.clone(), val);
                });
            })
            reset_key=Signal::derive(move || "wizard-static".to_string())
        />
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

                            view! {
                                <tr>
                                    <td><span class="badge badge-info">{label}</span></td>
                                    <td><code>{q.filename.clone()}</code></td>
                                    <td>
                                        <ContextFileDropdown
                                            filename=fname
                                            context_lengths
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
