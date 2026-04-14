use crate::components::pull_wizard::*;
use std::collections::HashSet;

#[component]
pub fn SelectionStep(
    repo_id: Signal<String>,
    available_quants: Signal<Vec<QuantEntry>>,
    available_mmprojs: Signal<Vec<QuantEntry>>,
    selected_filenames: RwSignal<HashSet<String>>,
    selected_mmproj_filenames: RwSignal<HashSet<String>>,
    on_next: Callback<()>,
    on_back: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Select Quantisations"</h2>
            <p class="form-card__desc text-muted">
                "Choose one or more quantisation files to download from "
                <code>{move || repo_id.get()}</code>"."
            </p>
        </div>

        <div class="form-actions mb-2">
            <button class="btn btn-secondary btn-sm" on:click=move |_| {
                let all: HashSet<String> = available_quants.get()
                    .iter()
                    .map(|q| q.filename.clone())
                    .collect();
                selected_filenames.set(all);
            }>
                "Select All"
            </button>
            <button class="btn btn-secondary btn-sm" on:click=move |_| {
                selected_filenames.set(HashSet::new());
            }>
                "Deselect All"
            </button>
        </div>

        <table class="data-table">
            <thead>
                <tr>
                    <th class="icon-sm"></th>
                    <th>"Quant"</th>
                    <th>"Filename"</th>
                    <th>"Size"</th>
                </tr>
            </thead>
            <tbody>
                {move || available_quants.get().into_iter().map(|q| {
                    let fname = q.filename.clone();
                    let fname_check = fname.clone();
                    let label = q.quant.clone().unwrap_or_else(|| fname.clone());
                    let size_str = q.size_bytes
                        .map(format_bytes)
                        .unwrap_or_else(|| "?".to_string());
                    let is_checked = move || selected_filenames.get().contains(&fname_check);
                    view! {
                        <tr>
                            <td>
                                <input
                                    type="checkbox"
                                    prop:checked=is_checked
                                    on:change=move |_| {
                                        selected_filenames.update(|set| {
                                            if set.contains(&fname) {
                                                set.remove(&fname);
                                            } else {
                                                set.insert(fname.clone());
                                            }
                                        });
                                    }
                                />
                            </td>
                            <td>
                                <span class="badge badge-info">{label}</span>
                            </td>
                            <td><code>{q.filename.clone()}</code></td>
                            <td class="text-muted">{size_str}</td>
                        </tr>
                    }
                }).collect::<Vec<_>>()}
            </tbody>
        </table>

        <div class="mt-4 mb-2">
            <h3 class="form-label">"Vision Projectors"</h3>
            <p class="text-muted text-sm mb-2">"Select vision projectors (mmproj) for this model."</p>
            <table class="data-table">
                <thead>
                    <tr>
                        <th class="icon-sm"></th>
                        <th>"Filename"</th>
                        <th>"Size"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || available_mmprojs.get().into_iter().map(|q| {
                        let fname = q.filename.clone();
                        let fname_check = fname.clone();
                        let size_str = q.size_bytes
                            .map(format_bytes)
                            .unwrap_or_else(|| "?".to_string());
                        let is_checked = move || selected_mmproj_filenames.get().contains(&fname_check);
                        view! {
                            <tr>
                                <td>
                                    <input
                                        type="checkbox"
                                        prop:checked=is_checked
                                        on:change=move |_| {
                                            selected_mmproj_filenames.update(|set| {
                                                if set.contains(&fname) {
                                                    set.remove(&fname);
                                                } else {
                                                    set.insert(fname.clone());
                                                }
                                            });
                                        }
                                    />
                                </td>
                                <td><code>{q.filename.clone()}</code></td>
                                <td class="text-muted">{size_str}</td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>

        <div class="form-actions mt-3">
            <Show when=move || !repo_id.get().trim().is_empty()>
                <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                    "Back"
                </button>
            </Show>
            <button
                class="btn btn-primary"
                prop:disabled=move || selected_filenames.get().is_empty() && selected_mmproj_filenames.get().is_empty()
                on:click=move |_| on_next.run(())
            >
                "Next →"
            </button>
        </div>
    }
}
