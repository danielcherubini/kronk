use leptos::prelude::*;

use super::types::{ModelForm, SamplingField};
use crate::utils::target_value;

#[component]
pub fn ModelEditorSamplingForm(
    form: RwSignal<Option<ModelForm>>,
    templates: LocalResource<Option<std::collections::HashMap<String, serde_json::Value>>>,
    load_preset_action: Action<String, (), LocalStorage>,
) -> impl IntoView {
    view! {
        <div class="form-grid">
            // Load Preset
            <label class="form-label" for="field-profile">"Load Preset"</label>
            <select
                id="field-profile"
                class="form-select"
                on:change=move |e| {
                    let name = target_value(&e);
                    if !name.is_empty() {
                        load_preset_action.dispatch(name);
                    }
                }
            >
                <option value="">"(select a preset)"</option>
                {move || {
                    if let Some(guard) = templates.get() {
                        if let Some(templates_map) = &*guard {
                            templates_map.keys().cloned().map(|k| {
                                let k_clone = k.clone();
                                view! { <option value=k_clone>{k}</option> }
                            }).collect::<Vec<_>>()
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    }
                }}
            </select>

            // Temperature
            <label class="form-label">
                <input
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().and_then(|f| f.sampling.get("temperature")).map(|f| f.enabled).unwrap_or(false)
                    on:change=move |e| {
                        use wasm_bindgen::JsCast;
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.sampling
                                    .entry("temperature".into())
                                    .or_insert_with(SamplingField::default)
                                    .enabled = checked;
                            }
                        });
                    }
                />
                "Temperature"
            </label>
            <input
                class="form-input"
                type="number"
                step="0.01"
                placeholder="0.3"
                prop:value=move || form.get().as_ref().and_then(|f| f.sampling.get("temperature")).map(|f| f.value.clone()).unwrap_or_default()
                prop:disabled=move || form.get().as_ref().and_then(|f| f.sampling.get("temperature")).map(|f| !f.enabled).unwrap_or(true)
                on:input=move |e| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.sampling
                                .entry("temperature".into())
                                .or_insert_with(SamplingField::default)
                                .value = target_value(&e);
                        }
                    });
                }
            />

            // Top K
            <label class="form-label">
                <input
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().and_then(|f| f.sampling.get("top_k")).map(|f| f.enabled).unwrap_or(false)
                    on:change=move |e| {
                        use wasm_bindgen::JsCast;
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.sampling
                                    .entry("top_k".into())
                                    .or_insert_with(SamplingField::default)
                                    .enabled = checked;
                            }
                        });
                    }
                />
                "Top K"
            </label>
            <input
                class="form-input"
                type="number"
                placeholder="40"
                prop:value=move || form.get().as_ref().and_then(|f| f.sampling.get("top_k")).map(|f| f.value.clone()).unwrap_or_default()
                prop:disabled=move || form.get().as_ref().and_then(|f| f.sampling.get("top_k")).map(|f| !f.enabled).unwrap_or(true)
                on:input=move |e| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.sampling
                                .entry("top_k".into())
                                .or_insert_with(SamplingField::default)
                                .value = target_value(&e);
                        }
                    });
                }
            />

            // Top P
            <label class="form-label">
                <input
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().and_then(|f| f.sampling.get("top_p")).map(|f| f.enabled).unwrap_or(false)
                    on:change=move |e| {
                        use wasm_bindgen::JsCast;
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.sampling
                                    .entry("top_p".into())
                                    .or_insert_with(SamplingField::default)
                                    .enabled = checked;
                            }
                        });
                    }
                />
                "Top P"
            </label>
            <input
                class="form-input"
                type="number"
                step="0.01"
                placeholder="0.9"
                prop:value=move || form.get().as_ref().and_then(|f| f.sampling.get("top_p")).map(|f| f.value.clone()).unwrap_or_default()
                prop:disabled=move || form.get().as_ref().and_then(|f| f.sampling.get("top_p")).map(|f| !f.enabled).unwrap_or(true)
                on:input=move |e| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.sampling
                                .entry("top_p".into())
                                .or_insert_with(SamplingField::default)
                                .value = target_value(&e);
                        }
                    });
                }
            />

            // Min P
            <label class="form-label">
                <input
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().and_then(|f| f.sampling.get("min_p")).map(|f| f.enabled).unwrap_or(false)
                    on:change=move |e| {
                        use wasm_bindgen::JsCast;
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.sampling
                                    .entry("min_p".into())
                                    .or_insert_with(SamplingField::default)
                                    .enabled = checked;
                            }
                        });
                    }
                />
                "Min P"
            </label>
            <input
                class="form-input"
                type="number"
                step="0.01"
                placeholder="0.05"
                prop:value=move || form.get().as_ref().and_then(|f| f.sampling.get("min_p")).map(|f| f.value.clone()).unwrap_or_default()
                prop:disabled=move || form.get().as_ref().and_then(|f| f.sampling.get("min_p")).map(|f| !f.enabled).unwrap_or(true)
                on:input=move |e| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.sampling
                                .entry("min_p".into())
                                .or_insert_with(SamplingField::default)
                                .value = target_value(&e);
                        }
                    });
                }
            />

            // Presence Penalty
            <label class="form-label">
                <input
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().and_then(|f| f.sampling.get("presence_penalty")).map(|f| f.enabled).unwrap_or(false)
                    on:change=move |e| {
                        use wasm_bindgen::JsCast;
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.sampling
                                    .entry("presence_penalty".into())
                                    .or_insert_with(SamplingField::default)
                                    .enabled = checked;
                            }
                        });
                    }
                />
                "Presence Penalty"
            </label>
            <input
                class="form-input"
                type="number"
                step="0.01"
                placeholder="0.1"
                prop:value=move || form.get().as_ref().and_then(|f| f.sampling.get("presence_penalty")).map(|f| f.value.clone()).unwrap_or_default()
                prop:disabled=move || form.get().as_ref().and_then(|f| f.sampling.get("presence_penalty")).map(|f| !f.enabled).unwrap_or(true)
                on:input=move |e| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.sampling
                                .entry("presence_penalty".into())
                                .or_insert_with(SamplingField::default)
                                .value = target_value(&e);
                        }
                    });
                }
            />

            // Frequency Penalty
            <label class="form-label">
                <input
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().and_then(|f| f.sampling.get("frequency_penalty")).map(|f| f.enabled).unwrap_or(false)
                    on:change=move |e| {
                        use wasm_bindgen::JsCast;
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.sampling
                                    .entry("frequency_penalty".into())
                                    .or_insert_with(SamplingField::default)
                                    .enabled = checked;
                            }
                        });
                    }
                />
                "Frequency Penalty"
            </label>
            <input
                class="form-input"
                type="number"
                step="0.01"
                placeholder="0.1"
                prop:value=move || form.get().as_ref().and_then(|f| f.sampling.get("frequency_penalty")).map(|f| f.value.clone()).unwrap_or_default()
                prop:disabled=move || form.get().as_ref().and_then(|f| f.sampling.get("frequency_penalty")).map(|f| !f.enabled).unwrap_or(true)
                on:input=move |e| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.sampling
                                .entry("frequency_penalty".into())
                                .or_insert_with(SamplingField::default)
                                .value = target_value(&e);
                        }
                    });
                }
            />

            // Repeat Penalty
            <label class="form-label">
                <input
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().and_then(|f| f.sampling.get("repeat_penalty")).map(|f| f.enabled).unwrap_or(false)
                    on:change=move |e| {
                        use wasm_bindgen::JsCast;
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.sampling
                                    .entry("repeat_penalty".into())
                                    .or_insert_with(SamplingField::default)
                                    .enabled = checked;
                            }
                        });
                    }
                />
                "Repeat Penalty"
            </label>
            <input
                class="form-input"
                type="number"
                step="0.01"
                placeholder="1.1"
                prop:value=move || form.get().as_ref().and_then(|f| f.sampling.get("repeat_penalty")).map(|f| f.value.clone()).unwrap_or_default()
                prop:disabled=move || form.get().as_ref().and_then(|f| f.sampling.get("repeat_penalty")).map(|f| !f.enabled).unwrap_or(true)
                on:input=move |e| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.sampling
                                .entry("repeat_penalty".into())
                                .or_insert_with(SamplingField::default)
                                .value = target_value(&e);
                        }
                    });
                }
            />
        </div>
    }
}
