use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use web_sys;

use crate::components::modal::Modal;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};

// Helper to convert RwSignal to Signal for Modal
fn rw_signal_to_signal<T: Clone + Send + Sync + 'static>(sig: RwSignal<T>) -> Signal<T> {
    let (read, _) = sig.split();
    read.into()
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// What kind of file a quant entry represents. Mirrors `koji_core::config::QuantKind`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuantKind {
    #[default]
    Model,
    Mmproj,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuantInfo {
    pub file: String,
    #[serde(default)]
    pub kind: QuantKind,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub context_length: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDetail {
    pub id: String,
    pub backend: String,
    pub model: Option<String>,
    pub quant: Option<String>,
    #[serde(default)]
    pub mmproj: Option<String>,
    pub args: Vec<String>,
    pub sampling: Option<serde_json::Value>,
    pub enabled: bool,
    pub context_length: Option<u32>,
    pub port: Option<u16>,
    pub display_name: Option<String>,
    pub gpu_layers: Option<u32>,
    pub quants: BTreeMap<String, QuantInfo>,
    pub backends: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResponse {
    pub models: Vec<serde_json::Value>,
    pub backends: Vec<String>,
    pub sampling_templates: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SamplingField {
    pub enabled: bool,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelForm {
    pub id: String,
    pub backend: String,
    pub model: Option<String>,
    pub quant: Option<String>,
    pub mmproj: Option<String>,
    pub args: Vec<String>,
    pub sampling: std::collections::HashMap<String, SamplingField>,
    pub enabled: bool,
    pub context_length: Option<u32>,
    pub port: Option<u16>,
    pub display_name: Option<String>,
    pub gpu_layers: Option<u32>,
    pub quants: BTreeMap<String, QuantInfo>,
}

// ── Data fetching ─────────────────────────────────────────────────────────────

async fn fetch_model(id: String) -> Option<ModelDetail> {
    if id == "new" {
        let resp = gloo_net::http::Request::get("/api/models")
            .send()
            .await
            .ok()?;
        let list: ModelListResponse = resp.json().await.ok()?;
        return Some(ModelDetail {
            id: String::new(),
            backend: list.backends.first().cloned().unwrap_or_default(),
            model: None,
            quant: None,
            args: vec![],
            sampling: None,
            enabled: true,
            context_length: None,
            port: None,
            display_name: None,
            gpu_layers: None,
            quants: BTreeMap::new(),
            backends: list.backends,
            mmproj: None,
        });
    }
    let resp = gloo_net::http::Request::get(&format!("/api/models/{}", id))
        .send()
        .await
        .ok()?;
    if resp.status() != 200 {
        return None;
    }
    resp.json::<ModelDetail>().await.ok()
}

fn form_to_sampling_json(form: &ModelForm) -> serde_json::Value {
    let mut obj = serde_json::Map::new();

    if let Some(field) = form.sampling.get("temperature") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("temperature".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("top_k") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<u64>() {
                obj.insert("top_k".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("top_p") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("top_p".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("min_p") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("min_p".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("presence_penalty") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("presence_penalty".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("frequency_penalty") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("frequency_penalty".to_string(), serde_json::json!(val));
            }
        }
    }
    if let Some(field) = form.sampling.get("repeat_penalty") {
        if field.enabled {
            if let Ok(val) = field.value.parse::<f64>() {
                obj.insert("repeat_penalty".to_string(), serde_json::json!(val));
            }
        }
    }

    if obj.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!(obj)
    }
}

// ── API calls ─────────────────────────────────────────────────────────────────

async fn save_model(form: ModelForm, is_new: bool) -> Result<(), String> {
    let sampling = form_to_sampling_json(&form);

    let body = serde_json::json!({
        "id": form.id,
        "backend": form.backend,
        "model": form.model,
        "quant": form.quant,
        "mmproj": form.mmproj,
        "args": form.args,
        "sampling": sampling,
        "enabled": form.enabled,
        "context_length": form.context_length,
        "port": form.port,
        "display_name": form.display_name,
        "gpu_layers": form.gpu_layers,
        "quants": form.quants,
    });

    let (url, method) = if is_new {
        ("/api/models".to_string(), "POST")
    } else {
        (format!("/api/models/{}", form.id), "PUT")
    };

    let req = if method == "POST" {
        gloo_net::http::Request::post(&url)
    } else {
        gloo_net::http::Request::put(&url)
    };

    let resp = req
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status() == 200 || resp.status() == 201 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(text)
    }
}

async fn rename_model(old_id: &str, new_id: &str) -> Result<(), String> {
    let body = serde_json::json!({ "new_id": new_id });
    let resp = gloo_net::http::Request::post(&format!("/api/models/{}/rename", old_id))
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status() == 200 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(text)
    }
}

async fn delete_model_api(id: String) -> Result<(), String> {
    let resp = gloo_net::http::Request::delete(&format!("/api/models/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(text)
    }
}

async fn fetch_sampling_templates() -> Option<std::collections::HashMap<String, serde_json::Value>>
{
    let resp = gloo_net::http::Request::get("/api/models")
        .send()
        .await
        .ok()?;
    let list: ModelListResponse = resp.json().await.ok()?;
    let templates = list.sampling_templates?;
    Some(templates)
}

// ── Component ─────────────────────────────────────────────────────────────────

#[component]
pub fn ModelEditor() -> impl IntoView {
    let params = use_params_map();
    let model_id = move || params.get().get("id").unwrap_or_default();
    let is_new = move || model_id() == "new";

    let detail = LocalResource::new(move || {
        let id = model_id();
        async move { fetch_model(id).await }
    });

    // Use LocalResource for templates
    let templates = LocalResource::new(|| async move { fetch_sampling_templates().await });

    // Form signals
    let form_id = RwSignal::new(String::new());
    let form_backend = RwSignal::new(String::new());
    let form_model = RwSignal::new(String::new());
    let form_quant = RwSignal::new(Option::<String>::None);
    let form_args = RwSignal::new(String::new());
    let form_enabled = RwSignal::new(true);
    let form_context_length = RwSignal::new(String::new());
    let form_port = RwSignal::new(String::new());
    let form_display_name = RwSignal::new(String::new());
    let form_gpu_layers = RwSignal::new(String::new());
    let form_vision_enabled = RwSignal::new(false);
    let available_mmprojs_for_select = RwSignal::new(Vec::<String>::new());
    let selected_mmproj_for_config = RwSignal::new(String::new());

    let sampling_fields: RwSignal<std::collections::HashMap<String, SamplingField>> =
        RwSignal::new(std::collections::HashMap::new());

    let backends = RwSignal::new(Vec::<String>::new());
    let quants: RwSignal<Vec<(String, QuantInfo)>> = RwSignal::new(vec![]);
    let pull_modal_open_signal = RwSignal::new(false);

    // Status
    let model_status = RwSignal::new(Option::<(bool, String)>::None);
    let deleted = RwSignal::new(false);
    let original_id = RwSignal::new(String::new());

    // Populate signals when resource loads
    Effect::new(move |_| {
        if let Some(guard) = detail.get() {
            if let Some(d) = guard.take() {
                backends.set(d.backends.clone());
                original_id.set(d.id.clone());
                form_id.set(d.id.clone());
                form_backend.set(d.backend.clone());
                form_model.set(d.model.unwrap_or_default());
                form_quant.set(d.quant);
                form_args.set(d.args.join("\n"));
                form_enabled.set(d.enabled);
                form_context_length
                    .set(d.context_length.map(|v| v.to_string()).unwrap_or_default());
                form_port.set(d.port.map(|v| v.to_string()).unwrap_or_default());
                form_display_name.set(d.display_name.unwrap_or_default());
                form_gpu_layers.set(d.gpu_layers.map(|v| v.to_string()).unwrap_or_default());

                // Load mmproj from model detail
                if let Some(mmproj) = d.mmproj.as_ref() {
                    form_vision_enabled.set(true);
                    selected_mmproj_for_config.set(mmproj.clone());
                }

                // Populate available mmprojs from d.quants by `kind`. The
                // dropdown values are quant *keys* (the BTreeMap keys), which
                // is what `ModelConfig.mmproj` references.
                let mmprojs: Vec<String> = d
                    .quants
                    .iter()
                    .filter(|(_, q)| q.kind == QuantKind::Mmproj)
                    .map(|(name, _)| name.clone())
                    .collect();
                available_mmprojs_for_select.set(mmprojs);

                let mut fields = std::collections::HashMap::new();
                if let Some(sampling_json) = &d.sampling {
                    if let Some(obj) = sampling_json.as_object() {
                        if let Some(temp) = obj.get("temperature") {
                            if let Some(val) = temp.as_f64() {
                                fields.insert(
                                    "temperature".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(top_k) = obj.get("top_k") {
                            if let Some(val) = top_k.as_u64() {
                                fields.insert(
                                    "top_k".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(top_p) = obj.get("top_p") {
                            if let Some(val) = top_p.as_f64() {
                                fields.insert(
                                    "top_p".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(min_p) = obj.get("min_p") {
                            if let Some(val) = min_p.as_f64() {
                                fields.insert(
                                    "min_p".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(presence) = obj.get("presence_penalty") {
                            if let Some(val) = presence.as_f64() {
                                fields.insert(
                                    "presence_penalty".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(frequency) = obj.get("frequency_penalty") {
                            if let Some(val) = frequency.as_f64() {
                                fields.insert(
                                    "frequency_penalty".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                        if let Some(repeat_pen) = obj.get("repeat_penalty") {
                            if let Some(val) = repeat_pen.as_f64() {
                                fields.insert(
                                    "repeat_penalty".to_string(),
                                    SamplingField {
                                        enabled: true,
                                        value: val.to_string(),
                                    },
                                );
                            }
                        }
                    }
                }
                sampling_fields.set(fields);

                let rows: Vec<(String, QuantInfo)> = d.quants.into_iter().collect();
                quants.set(rows);
            }
        }
    });

    let load_preset_action: Action<String, (), LocalStorage> =
        Action::new_unsync(move |preset_name: &String| {
            let preset_name_clone = preset_name.clone();
            async move {
                let templates_map = templates.get().and_then(|g| g.as_ref().cloned());
                if let Some(templates_map) = templates_map {
                    if let Some(preset) = templates_map.get(&preset_name_clone) {
                        if let Some(obj) = preset.as_object() {
                            let mut fields = sampling_fields.get().clone();

                            if let Some(temp) = obj.get("temperature") {
                                if let Some(val) = temp.as_f64() {
                                    fields
                                        .entry("temperature".to_string())
                                        .and_modify(|f| {
                                            f.enabled = true;
                                            f.value = val.to_string();
                                        })
                                        .or_insert(SamplingField {
                                            enabled: true,
                                            value: val.to_string(),
                                        });
                                }
                            }
                            if let Some(top_k) = obj.get("top_k") {
                                if let Some(val) = top_k.as_u64() {
                                    fields
                                        .entry("top_k".to_string())
                                        .and_modify(|f| {
                                            f.enabled = true;
                                            f.value = val.to_string();
                                        })
                                        .or_insert(SamplingField {
                                            enabled: true,
                                            value: val.to_string(),
                                        });
                                }
                            }
                            if let Some(top_p) = obj.get("top_p") {
                                if let Some(val) = top_p.as_f64() {
                                    fields
                                        .entry("top_p".to_string())
                                        .and_modify(|f| {
                                            f.enabled = true;
                                            f.value = val.to_string();
                                        })
                                        .or_insert(SamplingField {
                                            enabled: true,
                                            value: val.to_string(),
                                        });
                                }
                            }
                            if let Some(min_p) = obj.get("min_p") {
                                if let Some(val) = min_p.as_f64() {
                                    fields
                                        .entry("min_p".to_string())
                                        .and_modify(|f| {
                                            f.enabled = true;
                                            f.value = val.to_string();
                                        })
                                        .or_insert(SamplingField {
                                            enabled: true,
                                            value: val.to_string(),
                                        });
                                }
                            }
                            if let Some(presence) = obj.get("presence_penalty") {
                                if let Some(val) = presence.as_f64() {
                                    fields
                                        .entry("presence_penalty".to_string())
                                        .and_modify(|f| {
                                            f.enabled = true;
                                            f.value = val.to_string();
                                        })
                                        .or_insert(SamplingField {
                                            enabled: true,
                                            value: val.to_string(),
                                        });
                                }
                            }
                            if let Some(frequency) = obj.get("frequency_penalty") {
                                if let Some(val) = frequency.as_f64() {
                                    fields
                                        .entry("frequency_penalty".to_string())
                                        .and_modify(|f| {
                                            f.enabled = true;
                                            f.value = val.to_string();
                                        })
                                        .or_insert(SamplingField {
                                            enabled: true,
                                            value: val.to_string(),
                                        });
                                }
                            }
                            if let Some(repeat_pen) = obj.get("repeat_penalty") {
                                if let Some(val) = repeat_pen.as_f64() {
                                    fields
                                        .entry("repeat_penalty".to_string())
                                        .and_modify(|f| {
                                            f.enabled = true;
                                            f.value = val.to_string();
                                        })
                                        .or_insert(SamplingField {
                                            enabled: true,
                                            value: val.to_string(),
                                        });
                                }
                            }

                            sampling_fields.set(fields);
                        }
                    }
                }
            }
        });

    // Actions
    let _save_action: Action<(), (), LocalStorage> = Action::new_unsync(move |_: &()| {
        // Ensure form_id is set to original_id if empty (prevents creating new models)
        let save_id = if form_id.get().trim().is_empty() {
            original_id.get()
        } else {
            form_id.get()
        };
        // Args are passed through unchanged — the backend injects --mmproj
        // automatically when ModelConfig.mmproj is set, so the frontend must
        // not touch the args list.
        let args: Vec<String> = form_args
            .get()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        // Resolve the active mmproj key from the vision toggle. When the
        // toggle is off we send `None` so the backend clears the field.
        let mmproj =
            if form_vision_enabled.get() && !selected_mmproj_for_config.get().trim().is_empty() {
                Some(selected_mmproj_for_config.get())
            } else {
                None
            };

        let form = ModelForm {
            id: save_id,
            backend: form_backend.get(),
            model: if form_model.get().is_empty() {
                None
            } else {
                Some(form_model.get())
            },
            quant: form_quant.get(),
            mmproj,
            args,
            sampling: sampling_fields.get().clone(),
            enabled: form_enabled.get(),
            context_length: form_context_length.get().parse::<u32>().ok(),
            port: form_port.get().parse::<u16>().ok(),
            display_name: if form_display_name.get().is_empty() {
                None
            } else {
                Some(form_display_name.get())
            },
            gpu_layers: form_gpu_layers.get().parse::<u32>().ok(),
            quants: quants
                .get()
                .iter()
                .filter(|(n, _)| !n.trim().is_empty())
                .map(|(n, q)| (n.clone(), q.clone()))
                .collect(),
        };

        async move {
            let new_id = form.id.clone();
            let old_id = original_id.get();

            if old_id != new_id && !old_id.is_empty() {
                match rename_model(&old_id, &new_id).await {
                    Ok(()) => (),
                    Err(e) => {
                        model_status.set(Some((false, format!("Rename failed: {}", e))));
                        return;
                    }
                }
            }

            let form_id = form.id.clone();
            match save_model(form, is_new()).await {
                Ok(()) => {
                    original_id.set(form_id);
                    model_status.set(Some((true, "Saved.".into())));
                }
                Err(e) => {
                    if old_id != new_id && !old_id.is_empty() {
                        match rename_model(&new_id, &old_id).await {
                            Ok(()) => {
                                original_id.set(old_id.clone());
                                model_status
                                    .set(Some((false, format!("Save failed, rolled back: {}", e))));
                            }
                            Err(rename_err) => {
                                model_status.set(Some((
                                    false,
                                    format!(
                                        "Save failed ({}), and rollback also failed ({})",
                                        e, rename_err
                                    ),
                                )));
                            }
                        }
                    } else {
                        model_status.set(Some((false, format!("Error: {}", e))));
                    }
                }
            }
        }
    });

    let delete_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            match delete_model_api(form_id.get()).await {
                Ok(()) => deleted.set(true),
                Err(e) => model_status.set(Some((false, format!("Delete failed: {}", e)))),
            }
        });

    // View
    view! {
        <div class="page-header">
            <h1>
                {move || {
                    if is_new() {
                        "New Model".to_string()
                    } else {
                        format!("Edit: {}", model_id())
                    }
                }}
            </h1>
            <a href="/models" class="btn btn-secondary btn-sm">"← Back to Models"</a>
        </div>

        {move || deleted.get().then(|| view! {
            <div class="alert alert--success mb-3">
                <span class="alert__icon">"✓"</span>
                <span>"Model deleted. " <A href="/models">"← Back to Models"</A></span>
            </div>
        })}

        <Suspense fallback=|| view! {
            <div class="spinner-container">
                <span class="spinner"></span>
                <span class="text-muted">"Loading model..."</span>
            </div>
        }>
            {move || {
                let _ = detail.get();
                view! {
                    <div class="form-card">
                        <h2 class="form-card__title">{move || {
                            if is_new() { "New Model".to_string() } else { format!("Edit Model: {}", model_id()) }
                        }}</h2>

                    <form on:submit={move |e| {
                        e.prevent_default();
                        _save_action.dispatch(());
                    }}>
                            <div class="form-grid">
                                <label class="form-label" for="field-id">"ID"</label>
                                <input
                                    id="field-id"
                                    class="form-input"
                                    type="text"
                                    placeholder="e.g. my-model"
                                    prop:value=move || form_id.get()
                                    on:input=move |e| form_id.set(event_target_value(&e))
                                />

                                <label class="form-label" for="field-backend">"Backend"</label>
                                <select
                                    id="field-backend"
                                    class="form-select"
                                    on:change=move |e| form_backend.set(event_target_value(&e))
                                >
                                    {move || backends.get().into_iter().map(|b| {
                                        let selected = b == form_backend.get();
                                        let b2 = b.clone();
                                        view! { <option value=b selected=selected>{b2}</option> }
                                    }).collect::<Vec<_>>()}
                                </select>

                                <label class="form-label" for="field-display-name">"Display Name"</label>
                                <input
                                    id="field-display-name"
                                    class="form-input"
                                    type="text"
                                    placeholder="e.g. My Awesome Model"
                                    prop:value=move || form_display_name.get()
                                    on:input=move |e| form_display_name.set(event_target_value(&e))
                                />

                                <label class="form-label" for="field-model">"Model (HF repo)"</label>
                                <input
                                    id="field-model"
                                    class="form-input"
                                    type="text"
                                    placeholder="e.g. unsloth/gemma-4-26B-A4B-it-GGUF"
                                    prop:value=move || form_model.get()
                                    on:input=move |e| form_model.set(event_target_value(&e))
                                />

                                <label class="form-label" for="field-quant">"Quant"</label>
                                <select
                                    id="field-quant"
                                    class="form-select"
                                    prop:value=move || form_quant.get()
                                    on:change=move |e| {
                                        let value = event_target_value(&e);
                                        if value.is_empty() {
                                            form_quant.set(None);
                                        } else {
                                            form_quant.set(Some(value));
                                        }
                                    }
                                >
                                    <option value="">"No quant selected"</option>
                                    {move || {
                                        let quants = quants.get();
                                        if quants.is_empty() {
                                            view! { <option disabled>"No quants available"</option> }.into_any()
                                        } else {
                                            view! {
                                                <For
                                                    each=move || {
                                                        let quants = quants.clone();
                                                        quants.into_iter().map(|(k, _)| k)
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
                                    prop:value=move || form_gpu_layers.get()
                                    on:input=move |e| form_gpu_layers.set(event_target_value(&e))
                                />

                                <label class="form-label" for="field-profile">"Load Preset"</label>
                       <select
                                    id="field-profile"
                                    class="form-select"
                                    on:change=move |e| {
                                        let name = event_target_value(&e);
                                        load_preset_action.dispatch(name);
                                    }
                                >
                                    <option value="">"(select a preset)"</option>
                                    {move || {
                                        if let Some(guard) = templates.get() {
                                            if let Some(templates_map) = guard.as_ref() {
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

                                <label class="form-label" for="field-ctx">"Context length"</label>
                                <input
                                    id="field-ctx"
                                    class="form-input"
                                    type="number"
                                    placeholder="leave blank for default"
                                    prop:value=move || form_context_length.get()
                                    on:input=move |e| form_context_length.set(event_target_value(&e))
                                />

                                <label class="form-label" for="field-port">"Port override"</label>
                                <input
                                    id="field-port"
                                    class="form-input"
                                    type="number"
                                    placeholder="leave blank for default"
                                    prop:value=move || form_port.get()
                                    on:input=move |e| form_port.set(event_target_value(&e))
                                />

                                <label class="form-label">"Enabled"</label>
                                <div class="form-check">
                                    <input
                                        id="field-enabled"
                                        type="checkbox"
                                        prop:checked=move || form_enabled.get()
                                        on:change=move |e| {
                                            use wasm_bindgen::JsCast;
                                            let checked = e.target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                .map(|el| el.checked())
                                                .unwrap_or(false);
                                            form_enabled.set(checked);
                                        }
                                    />
                                    <label class="form-check-label" for="field-enabled">"Enabled"</label>
                                </div>
                            </div>

                            <h3 class="form-section-title">"Sampling Parameters"</h3>
                            <div class="form-grid">
                                <label class="form-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || {
                                            sampling_fields.get().get("temperature").map(|f| f.enabled).unwrap_or(false)
                                        }
                                        on:change=move |e| {
                                            use wasm_bindgen::JsCast;
                                            let checked = e.target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                .map(|el| el.checked())
                                                .unwrap_or(false);
                                            sampling_fields.update(|fields| {
                                                fields.entry("temperature".into())
                                                    .or_insert(SamplingField::default())
                                                    .enabled = checked;
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
                                    prop:value=move || {
                                        sampling_fields.get().get("temperature").map(|f| f.value.clone()).unwrap_or_default()
                                    }
                                    prop:disabled=move || {
                                        sampling_fields.get().get("temperature").map(|f| !f.enabled).unwrap_or(true)
                                    }
                                    on:input=move |e| {
                                        sampling_fields.update(|fields| {
                                            fields.entry("temperature".into())
                                                 .or_insert(SamplingField::default())
                                                 .value = event_target_value(&e);
                                        });
                                    }
                                />

                                <label class="form-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || {
                                            sampling_fields.get().get("top_k").map(|f| f.enabled).unwrap_or(false)
                                        }
                                        on:change=move |e| {
                                            use wasm_bindgen::JsCast;
                                            let checked = e.target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                .map(|el| el.checked())
                                                .unwrap_or(false);
                                            sampling_fields.update(|fields| {
                                                fields.entry("top_k".into())
                                                    .or_insert(SamplingField::default())
                                                    .enabled = checked;
                                            });
                                        }
                                    />
                                    "Top K"
                                </label>
                                <input
                                    class="form-input"
                                    type="number"
                                    placeholder="40"
                                    prop:value=move || {
                                        sampling_fields.get().get("top_k").map(|f| f.value.clone()).unwrap_or_default()
                                    }
                                    prop:disabled=move || {
                                        sampling_fields.get().get("top_k").map(|f| !f.enabled).unwrap_or(true)
                                    }
                                  on:input=move |e| {
                                         sampling_fields.update(|fields| {
                                             fields.entry("top_k".into())
                                                 .or_insert(SamplingField::default())
                                                 .value = event_target_value(&e);
                                         });
                                     }
                                />

                                <label class="form-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || {
                                            sampling_fields.get().get("top_p").map(|f| f.enabled).unwrap_or(false)
                                        }
                                        on:change=move |e| {
                                            use wasm_bindgen::JsCast;
                                            let checked = e.target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                .map(|el| el.checked())
                                                .unwrap_or(false);
                                            sampling_fields.update(|fields| {
                                                fields.entry("top_p".into())
                                                    .or_insert(SamplingField::default())
                                                    .enabled = checked;
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
                                    prop:value=move || {
                                        sampling_fields.get().get("top_p").map(|f| f.value.clone()).unwrap_or_default()
                                    }
                                    prop:disabled=move || {
                                        sampling_fields.get().get("top_p").map(|f| !f.enabled).unwrap_or(true)
                                    }
                                  on:input=move |e| {
                                         sampling_fields.update(|fields| {
                                             fields.entry("top_p".into())
                                                 .or_insert(SamplingField::default())
                                                 .value = event_target_value(&e);
                                         });
                                     }
                                />

                                <label class="form-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || {
                                            sampling_fields.get().get("min_p").map(|f| f.enabled).unwrap_or(false)
                                        }
                                        on:change=move |e| {
                                            use wasm_bindgen::JsCast;
                                            let checked = e.target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                .map(|el| el.checked())
                                                .unwrap_or(false);
                                            sampling_fields.update(|fields| {
                                                fields.entry("min_p".into())
                                                    .or_insert(SamplingField::default())
                                                    .enabled = checked;
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
                                    prop:value=move || {
                                        sampling_fields.get().get("min_p").map(|f| f.value.clone()).unwrap_or_default()
                                    }
                                    prop:disabled=move || {
                                        sampling_fields.get().get("min_p").map(|f| !f.enabled).unwrap_or(true)
                                    }
                                 on:input=move |e| {
                                         sampling_fields.update(|fields| {
                                             fields.entry("min_p".into())
                                                 .or_insert(SamplingField::default())
                                                 .value = event_target_value(&e);
                                         });
                                     }
                                />

                                <label class="form-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || {
                                            sampling_fields.get().get("presence_penalty").map(|f| f.enabled).unwrap_or(false)
                                        }
                                        on:change=move |e| {
                                            use wasm_bindgen::JsCast;
                                            let checked = e.target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                .map(|el| el.checked())
                                                .unwrap_or(false);
                                            sampling_fields.update(|fields| {
                                                fields.entry("presence_penalty".into())
                                                    .or_insert(SamplingField::default())
                                                    .enabled = checked;
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
                                    prop:value=move || {
                                        sampling_fields.get().get("presence_penalty").map(|f| f.value.clone()).unwrap_or_default()
                                    }
                                    prop:disabled=move || {
                                        sampling_fields.get().get("presence_penalty").map(|f| !f.enabled).unwrap_or(true)
                                    }
                                   on:input=move |e| {
                                         sampling_fields.update(|fields| {
                                             fields.entry("presence_penalty".into())
                                                 .or_insert(SamplingField::default())
                                                 .value = event_target_value(&e);
                                         });
                                     }
                                />

                                <label class="form-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || {
                                            sampling_fields.get().get("frequency_penalty").map(|f| f.enabled).unwrap_or(false)
                                        }
                                        on:change=move |e| {
                                            use wasm_bindgen::JsCast;
                                            let checked = e.target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                .map(|el| el.checked())
                                                .unwrap_or(false);
                                            sampling_fields.update(|fields| {
                                                fields.entry("frequency_penalty".into())
                                                    .or_insert(SamplingField::default())
                                                    .enabled = checked;
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
                                    prop:value=move || {
                                        sampling_fields.get().get("frequency_penalty").map(|f| f.value.clone()).unwrap_or_default()
                                    }
                                    prop:disabled=move || {
                                        sampling_fields.get().get("frequency_penalty").map(|f| !f.enabled).unwrap_or(true)
                                    }
                                    on:input=move |e| {
                                         sampling_fields.update(|fields| {
                                             fields.entry("frequency_penalty".into())
                                                 .or_insert(SamplingField::default())
                                                 .value = event_target_value(&e);
                                         });
                                     }
                                />

                                <label class="form-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || {
                                            sampling_fields.get().get("repeat_penalty").map(|f| f.enabled).unwrap_or(false)
                                        }
                                        on:change=move |e| {
                                            use wasm_bindgen::JsCast;
                                            let checked = e.target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                .map(|el| el.checked())
                                                .unwrap_or(false);
                                            sampling_fields.update(|fields| {
                                                fields.entry("repeat_penalty".into())
                                                    .or_insert(SamplingField::default())
                                                    .enabled = checked;
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
                                    prop:value=move || {
                                        sampling_fields.get().get("repeat_penalty").map(|f| f.value.clone()).unwrap_or_default()
                                    }
                                    prop:disabled=move || {
                                        sampling_fields.get().get("repeat_penalty").map(|f| !f.enabled).unwrap_or(true)
                                    }
                                 on:input=move |e| {
                                         sampling_fields.update(|fields| {
                                             fields.entry("repeat_penalty".into())
                                                 .or_insert(SamplingField::default())
                                                 .value = event_target_value(&e);
                                         });
                                     }
                                />
                            </div>

                            <h3 class="form-section-title">"Quantizations"</h3>
                            <table class="quants-table">
                                <thead>
                                    <tr>
                                        <th>"Name"</th>
                                        <th>"File"</th>
                                        <th>"Size (bytes)"</th>
                                        <th>"Context length"</th>
                                        <th></th>
                                    </tr>
                                </thead>
                                <tbody>
                                   <For
                                         each=move || quants.get().into_iter()
                                             .filter(|(_, q)| q.kind == QuantKind::Model)
                                             .enumerate()
                                         key=|(_i, (_name, _))| _i.to_string()
                                         children=move |(_i, (name, q))| {
                                            let name_arc = Arc::new(name.clone());
                                            view! {
                                                <tr>
                                                    <td>{name.clone()}</td>
                                                    <td>
                                                        <input
                                                             class="form-input"
                                                             type="text"
                                                             placeholder="model-Q4_K_M.gguf"
                                                             prop:value=move || q.file.clone()
                                                             on:input={
                                                                 let name_ref = Arc::clone(&name_arc);
                                                                 move |e| {
                                                                     let file = event_target_value(&e);
                                                                     quants.update(|rows| {
                                                                         if let Some(pos) = rows.iter().position(|(n, _)| n.as_str() == name_ref.as_str()) {
                                                                             if let Some((_, ref mut qq)) = rows.get_mut(pos) {
                                                                                 qq.file = file;
                                                                             }
                                                                         }
                                                                     });
                                                                 }
                                                             }
                                                         />
                                                    </td>
                                                    <td>
                                                        <input
                                                             class="form-input"
                                                             type="number"
                                                             placeholder="optional"
                                                             prop:value=move || q.size_bytes.map(|v| v.to_string()).unwrap_or_default()
                                                             on:input={
                                                                 let name_ref = Arc::clone(&name_arc);
                                                                 move |e| {
                                                                     let size = event_target_value(&e).parse::<u64>().ok();
                                                                     quants.update(|rows| {
                                                                         if let Some(pos) = rows.iter().position(|(n, _)| n.as_str() == name_ref.as_str()) {
                                                                             if let Some((_, ref mut qq)) = rows.get_mut(pos) {
                                                                                 qq.size_bytes = size;
                                                                             }
                                                                         }
                                                                     });
                                                                 }
                                                             }
                                                         />
                                                    </td>
                                                   <td>
                                                        <input
                                                             class="form-input"
                                                             type="number"
                                                             placeholder="optional"
                                                             prop:value=move || q.context_length.map(|v| v.to_string()).unwrap_or_default()
                                                             on:input={
                                                                 let name_ref = Arc::clone(&name_arc);
                                                                 move |e| {
                                                                     let ctx = event_target_value(&e).parse::<u32>().ok();
                                                                     quants.update(|rows| {
                                                                         if let Some(pos) = rows.iter().position(|(n, _)| n.as_str() == name_ref.as_str()) {
                                                                             if let Some((_, ref mut qq)) = rows.get_mut(pos) {
                                                                                 qq.context_length = ctx;
                                                                             }
                                                                         }
                                                                     });
                                                                 }
                                                             }
                                                         />
                                                     </td>
                                                     <td>
                                                          <button
                                                              type="button"
                                                              class="btn btn-danger btn-sm"
                                                              on:click={
                                                                  let name_ref = Arc::clone(&name_arc);
                                                                  move |_| {
                                                                      quants.update(|rows| {
                                                                          if let Some(pos) = rows.iter().position(|(n, _)| n.as_str() == name_ref.as_str()) {
                                                                              rows.remove(pos);
                                                                          }
                                                                      });
                                                                  }
                                                              }
                                                          >"✕"</button>
                                                      </td>
                                                </tr>
                                            }
                                        }
                                    />
                                </tbody>
                            </table>
                            <div class="mt-1">
                                <span title=move || if form_model.get().trim().is_empty() {
                                    "Enter the HuggingFace repo above before pulling quants".to_string()
                                } else {
                                    "Pull a new quant from HuggingFace".to_string()
                                }>
                                    <button
                                        type="button"
                                        class="btn btn-primary btn-sm"
                                        prop:disabled=move || form_model.get().trim().is_empty()
                                        on:click=move |_| pull_modal_open_signal.set(true)
                                    >"+ Pull Quant"</button>
                                </span>
                            </div>

                            <h3 class="form-section-title">"Vision Projector"</h3>
                            <div class="form-check">
                                <input
                                    id="field-vision-enabled"
                                    type="checkbox"
                                    prop:checked=move || form_vision_enabled.get()
                                    on:change=move |e| {
                                        use wasm_bindgen::JsCast;
                                        let checked = e.target()
                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            .map(|el| el.checked())
                                            .unwrap_or(false);
                                        form_vision_enabled.set(checked);
                                    }
                                />
                                <label class="form-check-label" for="field-vision-enabled">"Enable Vision Projector"</label>
                            </div>

                            <div class="form-group" prop:style=move || {
                                if form_vision_enabled.get() { "display: block;" } else { "display: none;" }
                            }>
                                <label class="form-label" for="mmproj-select">"Select mmproj File"</label>
                                <select
                                    id="mmproj-select"
                                    class="form-select"
                                    prop:value=move || selected_mmproj_for_config.get()
                                    on:change=move |e| {
                                        selected_mmproj_for_config.set(event_target_value(&e));
                                    }
                                >
                                    <option value="">"(none)"</option>
                                    {move || available_mmprojs_for_select.get().into_iter().map(|m| {
                                        let mm = m.clone();
                                        view! { <option value=mm.clone()>{mm.clone()}</option> }
                                    }).collect::<Vec<_>>()}
                                </select>
                                <span class="form-hint">"Choose the mmproj file to use for vision support"</span>
                            </div>

                            <label class="form-label" for="field-args">"Extra args"</label>
                            <textarea
                                id="field-args"
                                class="form-textarea"
                                rows="6"
                                placeholder="One flag per line, e.g.:\n-fa 1\n-b 4096\n--mlock"
                                prop:value=move || form_args.get()
                                on:input=move |e| form_args.set(event_target_value(&e))
                            />
                            <span class="form-hint">"One flag per line, e.g. -fa 1, --mlock, or -b 4096. Quote values containing spaces: -m \"path with space/m.gguf\""</span>

                            <hr class="section-divider" />

                             <div class="form-actions">
                                  <button type="submit" class="btn btn-primary">"Save Model"</button>
                                  <A href="/models"><button type="button" class="btn btn-secondary">"Cancel"</button></A>
                             {move || (!is_new()).then(|| view! {
                                 <button
                                     type="button"
                                     class="btn btn-danger ml-auto"
                                     on:click=move |_| {
                                         let confirmed = web_sys::window()
                                             .and_then(|w| w.confirm_with_message("Delete this model? This cannot be undone.").ok())
                                             .unwrap_or(false);
                                         if confirmed { delete_action.dispatch(()); }
                                     }
                                 >"Delete Model"</button>
                             })}
                             </div>

                             {move || model_status.get().map(|(ok, msg)| {
                                let cls = if ok { "alert alert--success mt-2" } else { "alert alert--error mt-2" };
                                let icon = if ok { "✓" } else { "✕" };
                                view! {
                                    <div class=cls>
                                        <span class="alert__icon">{icon}</span>
                                        <span>{msg}</span>
                                    </div>
                                }
                            })}
                        </form>
                    </div>
                    <Modal
                        open=rw_signal_to_signal(pull_modal_open_signal)
                        on_close=Callback::new(move |_| pull_modal_open_signal.set(false))
                        title="Pull Quant from HuggingFace".to_string()
                    >
                        <PullQuantWizard
                            initial_repo=Signal::derive(move || form_model.get())
                            is_open=rw_signal_to_signal(pull_modal_open_signal)
                            on_complete=Callback::new(move |completed: Vec<CompletedQuant>| {
                                // Visibility for the silent-failure caveat in spec §8.7: if all
                                // quants in this session failed, log to console so the user has
                                // *some* trace after the modal auto-closes.
                                if completed.is_empty() {
                                    web_sys::console::warn_1(
                                        &"All pulled quants failed; nothing merged into the editor.".into(),
                                    );
                                }
                                quants.update(|rows| {
                                    for cq in completed {
                                        // Detect mmproj files by filename pattern (matches
                                        // the backend's QuantKind::from_filename logic).
                                        let lower = cq.filename.to_lowercase();
                                        let kind = if lower.starts_with("mmproj") && lower.ends_with(".gguf") {
                                            QuantKind::Mmproj
                                        } else {
                                            QuantKind::Model
                                        };
                                        let key = cq.quant.clone().unwrap_or_else(|| {
                                            // Infer quant from filename: try standard patterns first,
                                            // otherwise use last component after splitting by `-` or `_`
                                            let stem = cq.filename.trim_end_matches(".gguf");
                                            let quant_patterns = [
                                                "IQ2_XXS", "IQ3_XXS", "IQ1_S", "IQ1_M", "IQ2_XS", "IQ2_S",
                                                "IQ2_M", "IQ3_XS", "IQ3_S", "IQ3_M", "IQ4_XS", "IQ4_NL",
                                                "Q2_K_S", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q4_K_S", "Q4_K_M",
                                                "Q4_K_L", "Q5_K_S", "Q5_K_M", "Q5_K_L", "Q2_K_XL", "Q3_K_XL",
                                                "Q4_K_XL", "Q5_K_XL", "Q6_K_XL", "Q8_K_XL", "Q2_K", "Q3_K",
                                                "Q4_K", "Q5_K", "Q6_K", "Q4_0", "Q4_1", "Q5_0", "Q5_1",
                                                "Q6_0", "Q8_0", "Q8_1", "F16", "F32", "BF16",
                                            ];
                                            let stem_upper = stem.to_uppercase();
                                            let quant = quant_patterns.iter().find(|pattern| {
                                                stem_upper.ends_with(*pattern)
                                                    || stem_upper.contains(&format!("-{}", pattern))
                                                    || stem_upper.contains(&format!(".{}", pattern))
                                                    || stem_upper.contains(&format!("_{}", pattern))
                                            }).map(|s| s.to_string());
                                            quant.unwrap_or_else(|| {
                                                stem.split(|c: char| c == '-' || c == '_')
                                                    .last()
                                                    .unwrap_or("unknown")
                                                    .to_string()
                                            })
                                        });
                                        if let Some(pos) = rows.iter().position(|(k, _)| k == &key) {
                                            // Re-pull: overwrite filename and context_length
                                            // (the wizard's values reflect the user's latest intent).
                                            // Only overwrite size_bytes when we have a value —
                                            // never clobber a known size with None.
                                            let row = &mut rows[pos].1;
                                            row.file = cq.filename;
                                            row.kind = kind;
                                            // Only set context_length for model quants;
                                            // mmprojs don't use it.
                                            if kind == QuantKind::Model {
                                                row.context_length = Some(cq.context_length);
                                            }
                                            if cq.size_bytes.is_some() {
                                                row.size_bytes = cq.size_bytes;
                                            }
                                        } else {
                                            // New row.
                                            rows.push((key, QuantInfo {
                                                file: cq.filename,
                                                kind,
                                                size_bytes: cq.size_bytes,
                                                context_length: if kind == QuantKind::Model {
                                                    Some(cq.context_length)
                                                } else {
                                                    None
                                                },
                                            }));
                                        }
                                    }
                                });
                                pull_modal_open_signal.set(false);
                            })
                            on_close=Callback::new(move |_| pull_modal_open_signal.set(false))
                        />
                    </Modal>
                }.into_any()
            }}
        </Suspense>
    }
}
