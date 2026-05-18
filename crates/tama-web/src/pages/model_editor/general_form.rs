use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::{BackendOption, ModelForm};
use crate::components::context_length_selector::ContextLengthSelector;
use crate::utils::target_value;

const MODALITY_OPTIONS: &[(&str, &str)] = &[
    ("text", "Text"),
    ("image", "Image"),
    ("audio", "Audio"),
    ("video", "Video"),
    ("pdf", "PDF"),
];

const KV_QUANT_OPTIONS: &[&str] = &[
    "f32", "f16", "bf16", "q8_0", "q4_0", "q4_1", "iq4_nl", "q5_0", "q5_1",
];

fn set_input_value(id: &str, value: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    {
        if let Ok(input) = el.clone().dyn_into::<web_sys::HtmlInputElement>() {
            input.set_value(value);
            return;
        }
        if let Ok(select) = el.dyn_into::<web_sys::HtmlSelectElement>() {
            select.set_value(value);
        }
    }
}

/// Custom KV quant text input that appears when the selected value is not in the known options.
/// Shows when value is the "__custom" sentinel or any value not in KV_QUANT_OPTIONS.
#[component]
fn KvQuantCustomInput(form: RwSignal<Option<ModelForm>>, field: KvQuantField) -> impl IntoView {
    let is_custom = Signal::derive(move || {
        let f = form.get();
        let current = f.as_ref().and_then(|f| match field {
            KvQuantField::K => f.cache_type_k.as_deref(),
            KvQuantField::V => f.cache_type_v.as_deref(),
        });
        matches!(current, Some("__custom"))
            || matches!(current, Some(val) if !KV_QUANT_OPTIONS.contains(&val))
    });
    let current_value = Signal::derive(move || {
        let f = form.get();
        f.as_ref().and_then(|f| match field {
            KvQuantField::K => f.cache_type_k.clone(),
            KvQuantField::V => f.cache_type_v.clone(),
        })
    });
    view! {
        <Show when=move || is_custom.get()>
            {move || {
                let val = current_value.get().expect("KV quant custom value should be present when is_custom is true");
                view! {
                    <input class="form-input" type="text" maxlength="32" placeholder="Custom quant value..." prop:value=val on:input=move |ev| {
                        let v = target_value(&ev);
                        form.update(|f| {
                            if let Some(form) = f {
                                match field {
                                    KvQuantField::K => form.cache_type_k = if v.is_empty() { None } else { Some(v) },
                                    KvQuantField::V => form.cache_type_v = if v.is_empty() { None } else { Some(v) },
                                }
                            }
                        });
                    } />
                }
            }}
        </Show>
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KvQuantField {
    K,
    V,
}

#[component]
pub fn ModelEditorGeneralForm(
    form: RwSignal<Option<ModelForm>>,
    backends: RwSignal<Vec<BackendOption>>,
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
                set_input_value(
                    "field-num-parallel",
                    &f.num_parallel.map(|v| v.to_string()).unwrap_or_default(),
                );
                set_input_value(
                    "field-kv-quant-k",
                    f.cache_type_k.as_deref().unwrap_or_default(),
                );
                set_input_value(
                    "field-kv-quant-v",
                    f.cache_type_v.as_deref().unwrap_or_default(),
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
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            // Parse "name:variant" or just "name"
                            if let Some((name, variant)) = val.split_once(':') {
                                form.backend = name.to_string();
                                form.gpu_variant = Some(variant.to_string());
                            } else {
                                form.backend = val;
                                form.gpu_variant = None;
                            }
                        }
                    });
                }
            >
                {move || backends.get().into_iter().map(|opt| {
                    let value = if let Some(ref v) = opt.variant {
                        format!("{}:{}", opt.name, v)
                    } else {
                        opt.name.clone()
                    };
                    let selected = form.get_untracked().as_ref().map(|f| {
                        let expected = if let Some(ref v) = f.gpu_variant {
                            format!("{}:{}", f.backend, v)
                        } else {
                            f.backend.clone()
                        };
                        expected == value
                    }).unwrap_or(false);
                    let value2 = value.clone();
                    view! { <option value=value2 selected=selected>{opt.label.clone()}</option> }
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
                max_context=Signal::derive(move || form.get().and_then(|f| f.hf_context_length))
            />

            <label class="form-label" for="field-kv-quant-k">
                "KV cache type K"
                <div class="form-hint">Quantize the K cache to reduce VRAM usage. Lower precision = less memory, slightly slower inference.</div>
            </label>
            <select
                id="field-kv-quant-k"
                class="form-select"
                on:change=move |e| {
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.cache_type_k = if val.is_empty() { None } else { Some(val) };
                        }
                    });
                }
            >
                <option value="">"Default (f16)"</option>
                {KV_QUANT_OPTIONS.iter().map(|opt| {
                    let selected = form.get_untracked().as_ref()
                        .and_then(|f| f.cache_type_k.as_deref())
                        == Some(*opt);
                    let opt_str = *opt;
                    view! { <option value=opt_str selected=selected>{opt_str}</option> }
                }).collect::<Vec<_>>()}
                <option
                    value="__custom"
                    selected=move || form.get().as_ref()
                        .and_then(|f| f.cache_type_k.as_deref())
                        .map(|v| v == "__custom" || !KV_QUANT_OPTIONS.contains(&v))
                        .unwrap_or(false)
                >
                    "Custom…"
                </option>
            </select>
            <KvQuantCustomInput form=form field=KvQuantField::K />

            <label class="form-label" for="field-kv-quant-v">
                "KV cache type V"
                <div class="form-hint">Quantize the V cache to reduce VRAM usage. Lower precision = less memory, slightly slower inference.</div>
            </label>
            <select
                id="field-kv-quant-v"
                class="form-select"
                on:change=move |e| {
                    let val = target_value(&e);
                    form.update(|f| {
                        if let Some(form) = f {
                            form.cache_type_v = if val.is_empty() { None } else { Some(val) };
                        }
                    });
                }
            >
                <option value="">"Default (f16)"</option>
                {KV_QUANT_OPTIONS.iter().map(|opt| {
                    let selected = form.get_untracked().as_ref()
                        .and_then(|f| f.cache_type_v.as_deref())
                        == Some(*opt);
                    let opt_str = *opt;
                    view! { <option value=opt_str selected=selected>{opt_str}</option> }
                }).collect::<Vec<_>>()}
                <option
                    value="__custom"
                    selected=move || form.get().as_ref()
                        .and_then(|f| f.cache_type_v.as_deref())
                        .map(|v| v == "__custom" || !KV_QUANT_OPTIONS.contains(&v))
                        .unwrap_or(false)
                >
                    "Custom…"
                </option>
            </select>
            <KvQuantCustomInput form=form field=KvQuantField::V />

            <label class="form-label" for="field-num-parallel">"Num parallel slots"</label>
            <input
                id="field-num-parallel"
                class="form-input"
                type="number"
                min="1"
                placeholder="1"
                on:input=move |ev| {
                    form.update(|f| {
                        if let Some(form) = f {
                            form.num_parallel = target_value(&ev).parse::<u32>().ok();
                        }
                    });
                }
            />

            <label class="form-label" for="field-kv-unified">
                "Unified KV cache"
                <div class="form-hint">All parallel slots share a single context pool. Better for agent+subagent workflows.</div>
            </label>
            <div class="form-check">
                <input
                    id="field-kv-unified"
                    type="checkbox"
                    prop:checked=move || form.get().as_ref().map(|f| f.kv_unified).unwrap_or(true)
                    on:change=move |e| {
                        let checked = e.target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                            .map(|el| el.checked())
                            .unwrap_or(false);
                        form.update(|f| {
                            if let Some(form) = f {
                                form.kv_unified = checked;
                            }
                        });
                    }
                />
                <label class="form-check-label" for="field-kv-unified">"Unified KV cache"</label>
            </div>

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
