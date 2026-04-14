use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::ModelForm;
use crate::components::context_length_selector::ContextLengthSelector;
use crate::utils::target_value;

const MODALITY_OPTIONS: &[(&str, &str)] = &[
    ("text", "Text"),
    ("image", "Image"),
    ("audio", "Audio"),
    ("video", "Video"),
    ("pdf", "PDF"),
];

fn set_input_value(id: &str, value: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        el.set_value(value);
    }
}

#[component]
pub fn ModelEditorGeneralForm(
    form: RwSignal<Option<ModelForm>>,
    backends: RwSignal<Vec<String>>,
) -> impl IntoView {
    // Track the last form ID we've initialized inputs for
    let last_init_id = StoredValue::new(None::<String>);

    // When form changes to a new model, populate the input values
    Effect::new(move |_| {
        if let Some(f) = form.get() {
            if last_init_id.get_value() != Some(f.id.clone()) {
                set_input_value(
                    "field-display-name",
                    f.display_name.as_deref().unwrap_or_default(),
                );
                set_input_value("field-model", f.model.as_deref().unwrap_or_default());
                set_input_value(
                    "field-gpu-layers",
                    &f.gpu_layers.map(|v| v.to_string()).unwrap_or_default(),
                );
                set_input_value(
                    "field-ctx",
                    &f.context_length.map(|v| v.to_string()).unwrap_or_default(),
                );
                set_input_value(
                    "field-port",
                    &f.port.map(|v| v.to_string()).unwrap_or_default(),
                );
                last_init_id.set_value(Some(f.id.clone()));
            }
        }
    });

    view! {
        <div class="form-grid">
            <label class="form-label" for="field-display-name">"Display Name"</label>
            <input
                id="field-display-name"
                class="form-input"
                type="text"
                placeholder="Auto-generated from HF repo name"
                on:input=move |ev| {
                    let val = target_value(&ev);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.display_name = if val.is_empty() { None } else { Some(val) };
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
                disabled=true
                title="API Name is auto-derived from the HF repo name"
                prop:value=move || form.get().as_ref().and_then(|f| f.api_name.clone()).unwrap_or_default()
            />

            <label class="form-label" for="field-model">"Model (HF repo)"</label>
            <input
                id="field-model"
                class="form-input"
                type="text"
                placeholder="e.g. unsloth/gemma-4-26B-A4B-it-GGUF"
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            let val = target_value(&ev);
                            form.model = if val.is_empty() { None } else { Some(val) };
                        }
                    });
                }
            />

            <label class="form-label" for="field-gpu-layers">"GPU Layers"</label>
            <input
                id="field-gpu-layers"
                class="form-input"
                type="number"
                placeholder="e.g. 999"
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.gpu_layers = target_value(&ev).parse::<u32>().ok();
                        }
                    });
                }
            />

            <label class="form-label" for="field-ctx">"Context length"</label>
            <ContextLengthSelector
                value=Signal::derive(move || form.get().and_then(|f| f.context_length))
                on_change=Callback::new(move |v| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.context_length = v;
                        }
                    });
                })
                reset_key=Signal::derive(move || form.get().map(|f| f.id.clone()).unwrap_or_default())
            />

            <label class="form-label" for="field-port">"Port override"</label>
            <input
                id="field-port"
                class="form-input"
                type="number"
                placeholder="leave blank for default"
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

            <label class="form-label">"Input Modalities"</label>
            <div class="form-check-group">
                <For
                    each=move || MODALITY_OPTIONS.iter().enumerate().map(|(i, (v, l))| (i, *v, *l))
                    key=|(i, v, _)| (*i, v.to_string())
                    children=move |(_i, value, label)| {
                        let value_str = value.to_string();
                        let input_id = format!("field-modality-input-{}", value);
                        let label_for = format!("field-modality-input-{}", value);
                        let checked_value = value_str.clone();
                        let onchange_value = value_str.clone();
                        view! {
                            <div class="form-check">
                                <input
                                    id=input_id
                                    type="checkbox"
                                    prop:checked=move || {
                                        form.get()
                                            .as_ref()
                                            .and_then(|f| f.modalities.as_ref())
                                            .map(|m| m.input.contains(&checked_value))
                                            .unwrap_or(false)
                                    }
                                    on:change=move |e| {
                                        let checked = e.target()
                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            .map(|el| el.checked())
                                            .unwrap_or(false);
                                        let v = onchange_value.clone();
                                        form.update(move |f| {
                                            if let Some(form) = f {
                                                if let Some(m) = form.modalities.as_mut() {
                                                    if checked {
                                                        if !m.input.contains(&v) {
                                                            m.input.push(v.clone());
                                                        }
                                                    } else {
                                                        m.input.retain(|x| *x != v);
                                                    }
                                                }
                                            }
                                        });
                                    }
                                />
                                <label class="form-check-label" for=label_for>{label}</label>
                            </div>
                        }
                    }
                />
            </div>

            <label class="form-label">"Output Modalities"</label>
            <div class="form-check-group">
                <For
                    each=move || MODALITY_OPTIONS.iter().enumerate().map(|(i, (v, l))| (i, *v, *l))
                    key=|(i, v, _)| (*i, format!("out-{}", v))
                    children=move |(_i, value, label)| {
                        let value_str = value.to_string();
                        let input_id = format!("field-modality-output-{}", value);
                        let label_for = format!("field-modality-output-{}", value);
                        let checked_value = value_str.clone();
                        let onchange_value = value_str.clone();
                        view! {
                            <div class="form-check">
                                <input
                                    id=input_id
                                    type="checkbox"
                                    prop:checked=move || {
                                        form.get()
                                            .as_ref()
                                            .and_then(|f| f.modalities.as_ref())
                                            .map(|m| m.output.contains(&checked_value))
                                            .unwrap_or(false)
                                    }
                                    on:change=move |e| {
                                        let checked = e.target()
                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            .map(|el| el.checked())
                                            .unwrap_or(false);
                                        let v = onchange_value.clone();
                                        form.update(move |f| {
                                            if let Some(form) = f {
                                                if let Some(m) = form.modalities.as_mut() {
                                                    if checked {
                                                        if !m.output.contains(&v) {
                                                            m.output.push(v.clone());
                                                        }
                                                    } else {
                                                        m.output.retain(|x| *x != v);
                                                    }
                                                }
                                            }
                                        });
                                    }
                                />
                                <label class="form-check-label" for=label_for>{label}</label>
                            </div>
                        }
                    }
                />
            </div>
        </div>
    }
}
