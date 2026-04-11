use leptos::prelude::*;

use super::types::ModelForm;
use crate::utils::target_value;

#[component]
pub fn ModelEditorGeneralForm(
    form: RwSignal<Option<ModelForm>>,
    backends: RwSignal<Vec<String>>,
) -> impl IntoView {
    view! {
        <div class="form-grid">
            <label class="form-label" for="field-id">"ID"</label>
            <input
                id="field-id"
                class="form-input"
                type="text"
                placeholder="e.g. my-model"
                prop:value=move || form.get().as_ref().map(|f| f.id.clone()).unwrap_or_default()
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.id = target_value(&ev);
                        }
                    });
                }
            />

            <label class="form-label" for="field-backend">"Backend"</label>
            <select
                id="field-backend"
                class="form-select"
                on:change=move |e| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.backend = target_value(&e);
                        }
                    });
                }
            >
                {move || backends.get().into_iter().map(|b| {
                    let selected = form.get().as_ref().map(|f| f.backend.clone()).unwrap_or_default() == b;
                    let b2 = b.clone();
                    view! { <option value=b2 selected=selected>{b}</option> }
                }).collect::<Vec<_>>()}
            </select>

            <label class="form-label" for="field-api-name">"API Name"</label>
            <input
                id="field-api-name"
                class="form-input"
                type="text"
                placeholder="e.g. My Awesome Model"
                prop:value=move || form.get().as_ref().and_then(|f| f.api_name.clone()).unwrap_or_default()
                on:input=move |ev| {
                    let val = target_value(&ev);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.api_name = if val.is_empty() {
                                None
                            } else {
                                Some(val)
                            };
                        }
                    });
                }
            />

            <label class="form-label" for="field-model">"Model (HF repo)"</label>
            <input
                id="field-model"
                class="form-input"
                type="text"
                placeholder="e.g. unsloth/gemma-4-26B-A4B-it-GGUF"
                prop:value=move || form.get().as_ref().and_then(|f| f.model.clone()).unwrap_or_default()
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            let val = target_value(&ev);
                            form.model = if val.is_empty() { None } else { Some(val) };
                        }
                    });
                }
            />

            <label class="form-label" for="field-quant">"Quant"</label>
            <select
                id="field-quant"
                class="form-select"
                prop:value=move || form.get().as_ref().and_then(|f| f.quant.clone()).unwrap_or_default()
                on:change=move |e| {
                    let value = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.quant = if value.is_empty() { None } else { Some(value) };
                        }
                    });
                }
            >
                <option value="">"No quant selected"</option>
                {move || {
                    let quants = form.get().map(|f| f.quants.clone()).unwrap_or_default();
                    if quants.is_empty() {
                        view! { <option disabled>"No quants available"</option> }.into_any()
                    } else {
                        view! {
                            <For
                                each=move || {
                                    let quants = quants.clone();
                                    quants.into_keys()
                                }
                                key=|k| k.clone()
                                children=move |quant_key| {
                                    view! { <option value={quant_key.clone()}> {quant_key.clone()} </option> }.into_any()
                                }
                            />
                        }
                        .into_any()
                    }
                }}
            </select>

            <label class="form-label" for="field-gpu-layers">"GPU Layers"</label>
            <input
                id="field-gpu-layers"
                class="form-input"
                type="number"
                placeholder="e.g. 999"
                prop:value=move || form.get().as_ref().and_then(|f| f.gpu_layers).map(|v| v.to_string()).unwrap_or_default()
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.gpu_layers = target_value(&ev).parse::<u32>().ok();
                        }
                    });
                }
            />

            <label class="form-label" for="field-ctx">"Context length"</label>
            <input
                id="field-ctx"
                class="form-input"
                type="number"
                placeholder="leave blank for default"
                prop:value=move || form.get().as_ref().and_then(|f| f.context_length).map(|v| v.to_string()).unwrap_or_default()
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.context_length = target_value(&ev).parse::<u32>().ok();
                        }
                    });
                }
            />

            <label class="form-label" for="field-port">"Port override"</label>
            <input
                id="field-port"
                class="form-input"
                type="number"
                placeholder="leave blank for default"
                prop:value=move || form.get().as_ref().and_then(|f| f.port).map(|v| v.to_string()).unwrap_or_default()
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.port = target_value(&ev).parse::<u16>().ok();
                        }
                    });
                }
            />

            <label class="form-label">"Enabled"</label>
            <div class="form-check">
                <input
                    id="field-enabled"
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().map(|f| f.enabled).unwrap_or(true)
                    on:change=move |e| {
                        use wasm_bindgen::JsCast;
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.enabled = checked;
                            }
                        });
                    }
                />
                <label class="form-check-label" for="field-enabled">"Enabled"</label>
            </div>
        </div>
    }
}
