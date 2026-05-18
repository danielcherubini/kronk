use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::ModelForm;
use crate::utils::target_value;

/// Speculative decoding form section for the model editor.
#[component]
pub fn ModelEditorSpecDecodingForm(form: RwSignal<Option<ModelForm>>) -> impl IntoView {
    // Checkboxes for spec types
    let toggle_spec_type = move |e: web_sys::Event, spec_type: String| {
        let checked = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|el| el.checked())
            .unwrap_or(false);
        form.update(move |f| {
            if let Some(form) = f {
                if checked {
                    if !form.spec_decoding.spec_types.contains(&spec_type) {
                        form.spec_decoding.spec_types.push(spec_type);
                    }
                } else {
                    form.spec_decoding.spec_types.retain(|s| s != &spec_type);
                }
            }
        });
    };

    let has_any_type = Signal::derive(move || {
        form.get()
            .as_ref()
            .map(|f| !f.spec_decoding.spec_types.is_empty())
            .unwrap_or(false)
    });

    let has_draft_mtp = Signal::derive(move || {
        form.get()
            .as_ref()
            .map(|f| {
                f.spec_decoding
                    .spec_types
                    .contains(&"draft-mtp".to_string())
            })
            .unwrap_or(false)
    });

    view! {
        <div class="form-grid">
            // Spec type checkboxes
            <label class="form-label">"Speculative Decoding Types"</label>
            <div class="form-check-group">
                // draft-mtp checkbox
                <div class="form-check">
                    <input
                        id="field-spec-draft-mtp"
                        type="checkbox"
                        prop:checked=move || {
                            form.get()
                                .as_ref()
                                .map(|f| f.spec_decoding.spec_types.contains(&"draft-mtp".to_string()))
                                .unwrap_or(false)
                        }
                        on:change=move |e| {
                            toggle_spec_type(e, "draft-mtp".to_string());
                        }
                    />
                    <label class="form-check-label" for="field-spec-draft-mtp">
                        "draft-mtp"
                        <div class="form-hint">"Multi-Token Prediction — uses a draft model for speculative decoding"</div>
                    </label>
                </div>

                // ngram-simple checkbox
                <div class="form-check">
                    <input
                        id="field-spec-ngram-simple"
                        type="checkbox"
                        prop:checked=move || {
                            form.get()
                                .as_ref()
                                .map(|f| f.spec_decoding.spec_types.contains(&"ngram-simple".to_string()))
                                .unwrap_or(false)
                        }
                        on:change=move |e| {
                            toggle_spec_type(e, "ngram-simple".to_string());
                        }
                    />
                    <label class="form-check-label" for="field-spec-ngram-simple">
                        "ngram-simple"
                        <div class="form-hint">"Simple n-gram speculative decoding — lightweight, no extra model needed"</div>
                    </label>
                </div>
            </div>

            // Draft Max (n_max) — shown when any type is checked
            <Show when=move || has_any_type.get()>
                <label class="form-label" for="field-spec-n-max">"Draft Max"</label>
                <select
                    id="field-spec-n-max"
                    class="form-select"
                    on:change=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.spec_decoding.n_max = val.parse::<u32>().ok();
                            }
                        });
                    }
                >
                    <option value="">"(select)"</option>
                    {(1..=8).map(|v| {
                        let selected = form.get_untracked()
                            .as_ref()
                            .map(|f| f.spec_decoding.n_max == Some(v))
                            .unwrap_or(false);
                        let val = v.to_string();
                        view! { <option value=val selected=selected>{v}</option> }
                    }).collect::<Vec<_>>()}
                </select>

                // Draft Min (n_min) — shown when any type is checked
                <label class="form-label" for="field-spec-n-min">"Draft Min"</label>
                <select
                    id="field-spec-n-min"
                    class="form-select"
                    on:change=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.spec_decoding.n_min = val.parse::<u32>().ok();
                            }
                        });
                    }
                >
                    <option value="">"(select)"</option>
                    {(1..=8).map(|v| {
                        let selected = form.get_untracked()
                            .as_ref()
                            .map(|f| f.spec_decoding.n_min == Some(v))
                            .unwrap_or(false);
                        let val = v.to_string();
                        view! { <option value=val selected=selected>{v}</option> }
                    }).collect::<Vec<_>>()}
                </select>
            </Show>

            // Draft GPU Layers (draft_ngl) — shown when draft-mtp is checked
            <Show when=move || has_draft_mtp.get()>
                <label class="form-label" for="field-spec-draft-ngl">
                    "Draft GPU Layers"
                    <div class="form-hint">"99 = all layers"</div>
                </label>
                <input
                    id="field-spec-draft-ngl"
                    class="form-input"
                    type="number"
                    min="0"
                    max="999"
                    placeholder="e.g. 99"
                    on:input=move |e| {
                        let val = target_value(&e);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.spec_decoding.draft_ngl = if val.is_empty() {
                                    None
                                } else {
                                    val.parse::<u32>().ok()
                                };
                            }
                        });
                    }
                />
            </Show>
        </div>
    }
}
