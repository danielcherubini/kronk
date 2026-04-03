use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ModelForm {
    id: String,
    backend: String,
    model: String,
    quant: String,
    args: String, // newline-separated in the textarea
    profile: String,
    enabled: bool,
    context_length: String,
    port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CardForm {
    name: String,
    source: String,
    default_context_length: String,
    default_gpu_layers: String,
    quants_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuantInfo {
    file: String,
    size_bytes: Option<u64>,
    context_length: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelMeta {
    name: String,
    source: String,
    default_context_length: Option<u32>,
    default_gpu_layers: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CardData {
    model: ModelMeta,
    quants: HashMap<String, QuantInfo>,
    sampling: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelDetail {
    id: String,
    backend: String,
    model: Option<String>,
    quant: Option<String>,
    args: Vec<String>,
    profile: Option<String>,
    enabled: bool,
    context_length: Option<u32>,
    port: Option<u16>,
    backends: Vec<String>,
    card: Option<CardData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelListResponse {
    models: Vec<serde_json::Value>,
    backends: Vec<String>,
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
            profile: Some("coding".to_string()),
            enabled: true,
            context_length: None,
            port: None,
            backends: list.backends,
            card: None,
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

fn detail_to_form(d: &ModelDetail) -> ModelForm {
    ModelForm {
        id: d.id.clone(),
        backend: d.backend.clone(),
        model: d.model.clone().unwrap_or_default(),
        quant: d.quant.clone().unwrap_or_default(),
        args: d.args.join("\n"),
        profile: d.profile.clone().unwrap_or_default(),
        enabled: d.enabled,
        context_length: d.context_length.map(|v| v.to_string()).unwrap_or_default(),
        port: d.port.map(|v| v.to_string()).unwrap_or_default(),
    }
}

fn card_to_form(card: &CardData) -> CardForm {
    let quants_json = serde_json::to_string_pretty(&card.quants).unwrap_or_default();
    CardForm {
        name: card.model.name.clone(),
        source: card.model.source.clone(),
        default_context_length: card
            .model
            .default_context_length
            .map(|v| v.to_string())
            .unwrap_or_default(),
        default_gpu_layers: card
            .model
            .default_gpu_layers
            .map(|v| v.to_string())
            .unwrap_or_default(),
        quants_json,
    }
}

// ── API calls ─────────────────────────────────────────────────────────────────

async fn save_model(form: ModelForm, is_new: bool) -> Result<(), String> {
    let args: Vec<String> = form
        .args
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let body = serde_json::json!({
        "id": form.id,
        "backend": form.backend,
        "model": if form.model.is_empty() { serde_json::Value::Null } else { form.model.into() },
        "quant": if form.quant.is_empty() { serde_json::Value::Null } else { form.quant.into() },
        "args": args,
        "profile": if form.profile.is_empty() { serde_json::Value::Null } else { form.profile.into() },
        "enabled": form.enabled,
        "context_length": form.context_length.parse::<u32>().ok(),
        "port": form.port.parse::<u16>().ok(),
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

async fn save_card(model_id: String, form: CardForm) -> Result<(), String> {
    let quants: serde_json::Value = if form.quants_json.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&form.quants_json)
            .map_err(|e| format!("Invalid quants JSON: {}", e))?
    };

    let body = serde_json::json!({
        "name": form.name,
        "source": form.source,
        "default_context_length": form.default_context_length.parse::<u32>().ok(),
        "default_gpu_layers": form.default_gpu_layers.parse::<u32>().ok(),
        "quants": quants,
        "sampling": {},
    });

    let resp = gloo_net::http::Request::put(&format!("/api/models/{}/card", model_id))
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

    // ── Model config form signals ─────────────────────────────────────────────
    let form_id = RwSignal::new(String::new());
    let form_backend = RwSignal::new(String::new());
    let form_model = RwSignal::new(String::new());
    let form_quant = RwSignal::new(String::new());
    let form_args = RwSignal::new(String::new());
    let form_profile = RwSignal::new(String::new());
    let form_enabled = RwSignal::new(true);
    let form_context_length = RwSignal::new(String::new());
    let form_port = RwSignal::new(String::new());
    let backends = RwSignal::new(Vec::<String>::new());

    // ── Card form signals ─────────────────────────────────────────────────────
    let card_name = RwSignal::new(String::new());
    let card_source = RwSignal::new(String::new());
    let card_default_ctx = RwSignal::new(String::new());
    let card_default_gpu = RwSignal::new(String::new());
    let card_quants_json = RwSignal::new(String::new());
    let has_card = RwSignal::new(false);

    // ── Status ────────────────────────────────────────────────────────────────
    let model_status = RwSignal::new(Option::<(bool, String)>::None);
    let card_status = RwSignal::new(Option::<(bool, String)>::None);
    let deleted = RwSignal::new(false);

    // Populate form when resource loads
    Effect::new(move |_| {
        if let Some(guard) = detail.get() {
            if let Some(d) = guard.take() {
                backends.set(d.backends.clone());
                let f = detail_to_form(&d);
                form_id.set(f.id);
                form_backend.set(f.backend);
                form_model.set(f.model.clone());
                form_quant.set(f.quant);
                form_args.set(f.args);
                form_profile.set(f.profile);
                form_enabled.set(f.enabled);
                form_context_length.set(f.context_length);
                form_port.set(f.port);

                if let Some(card) = &d.card {
                    has_card.set(true);
                    let cf = card_to_form(card);
                    card_name.set(cf.name);
                    card_source.set(cf.source);
                    card_default_ctx.set(cf.default_context_length);
                    card_default_gpu.set(cf.default_gpu_layers);
                    card_quants_json.set(cf.quants_json);
                } else {
                    has_card.set(false);
                    card_source.set(f.model);
                }
            }
        }
    });

    let save_model_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            let form = ModelForm {
                id: form_id.get(),
                backend: form_backend.get(),
                model: form_model.get(),
                quant: form_quant.get(),
                args: form_args.get(),
                profile: form_profile.get(),
                enabled: form_enabled.get(),
                context_length: form_context_length.get(),
                port: form_port.get(),
            };
            match save_model(form, is_new()).await {
                Ok(()) => model_status.set(Some((true, "Saved.".into()))),
                Err(e) => model_status.set(Some((false, format!("Error: {}", e)))),
            }
        });

    let save_card_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            let id = form_id.get();
            if id.is_empty() {
                card_status.set(Some((false, "Save the model config first.".into())));
                return;
            }
            let form = CardForm {
                name: card_name.get(),
                source: card_source.get(),
                default_context_length: card_default_ctx.get(),
                default_gpu_layers: card_default_gpu.get(),
                quants_json: card_quants_json.get(),
            };
            match save_card(id, form).await {
                Ok(()) => {
                    has_card.set(true);
                    card_status.set(Some((true, "Card saved.".into())));
                }
                Err(e) => card_status.set(Some((false, format!("Error: {}", e)))),
            }
        });

    let delete_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            match delete_model_api(form_id.get()).await {
                Ok(()) => deleted.set(true),
                Err(e) => model_status.set(Some((false, format!("Delete failed: {}", e)))),
            }
        });

    view! {
        <h1>{move || if is_new() { "New Model".to_string() } else { format!("Edit: {}", model_id()) }}</h1>

        {move || deleted.get().then(|| view! {
            <p style="color: green">
                "Model deleted. " <A href="/models">"← Back to Models"</A>
            </p>
        })}

        <Suspense fallback=|| view! { <p>"Loading..."</p> }>
            {move || {
                let _ = detail.get();
                view! {
                    <div>

                        // ── Model Config ──────────────────────────────────────
                        <h2>"Model Config"</h2>
                        <form on:submit=move |e| { e.prevent_default(); save_model_action.dispatch(()); }>
                            <table style="border-collapse: collapse; width: 100%;">
                                <tr>
                                    <td style="padding: 6px; font-weight: bold; width: 160px;">"ID"</td>
                                    <td style="padding: 6px;">
                                        <input type="text" style="width: 100%;"
                                            placeholder="e.g. my-model"
                                            prop:value=move || form_id.get()
                                            prop:disabled=move || !is_new()
                                            on:input=move |e| form_id.set(event_target_value(&e))
                                        />
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Backend"</td>
                                    <td style="padding: 6px;">
                                        <select style="width: 100%;"
                                            on:change=move |e| form_backend.set(event_target_value(&e))
                                        >
                                            {move || backends.get().into_iter().map(|b| {
                                                let selected = b == form_backend.get();
                                                let b2 = b.clone();
                                                view! { <option value=b selected=selected>{b2}</option> }
                                            }).collect::<Vec<_>>()}
                                        </select>
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Model (HF repo)"</td>
                                    <td style="padding: 6px;">
                                        <input type="text" style="width: 100%;"
                                            placeholder="e.g. unsloth/gemma-4-26B-A4B-it-GGUF"
                                            prop:value=move || form_model.get()
                                            on:input=move |e| form_model.set(event_target_value(&e))
                                        />
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Quant"</td>
                                    <td style="padding: 6px;">
                                        <input type="text" style="width: 100%;"
                                            placeholder="e.g. Q4_K_M"
                                            prop:value=move || form_quant.get()
                                            on:input=move |e| form_quant.set(event_target_value(&e))
                                        />
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Profile"</td>
                                    <td style="padding: 6px;">
                                        <select style="width: 100%;"
                                            on:change=move |e| form_profile.set(event_target_value(&e))
                                        >
                                            {["", "coding", "chat", "analysis", "creative"].into_iter().map(|p| {
                                                let selected = p == form_profile.get();
                                                view! {
                                                    <option value=p selected=selected>
                                                        {if p.is_empty() { "(none)" } else { p }}
                                                    </option>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </select>
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Context length"</td>
                                    <td style="padding: 6px;">
                                        <input type="number" style="width: 100%;"
                                            placeholder="leave blank for default"
                                            prop:value=move || form_context_length.get()
                                            on:input=move |e| form_context_length.set(event_target_value(&e))
                                        />
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Port override"</td>
                                    <td style="padding: 6px;">
                                        <input type="number" style="width: 100%;"
                                            placeholder="leave blank for default"
                                            prop:value=move || form_port.get()
                                            on:input=move |e| form_port.set(event_target_value(&e))
                                        />
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Enabled"</td>
                                    <td style="padding: 6px;">
                                        <input type="checkbox"
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
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold; vertical-align: top;">"Extra args"</td>
                                    <td style="padding: 6px;">
                                        <textarea rows="6" style="width: 100%; font-family: monospace; font-size: 0.85em;"
                                            placeholder="One flag per line, e.g.:\n-ctk\nq4_0"
                                            prop:value=move || form_args.get()
                                            on:input=move |e| form_args.set(event_target_value(&e))
                                        />
                                        <small>"One argument per line (same as TOML args array)"</small>
                                    </td>
                                </tr>
                            </table>

                            <div style="margin-top: 0.75em; display: flex; gap: 0.5em; align-items: center;">
                                <button type="submit">"Save Model Config"</button>
                                <A href="/models"><button type="button">"Cancel"</button></A>
                                {move || (!is_new()).then(|| view! {
                                    <button type="button"
                                        style="margin-left: auto; background: #c0392b; color: white; border: none; padding: 0.4em 1em; cursor: pointer;"
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
                                let color = if ok { "green" } else { "red" };
                                view! { <p style=format!("color: {}", color)>{msg}</p> }
                            })}
                        </form>

                        // ── Model Card ────────────────────────────────────────
                        <hr style="margin: 2em 0;" />
                        <h2>
                            "Model Card "
                            {move || if has_card.get() {
                                let filename = form_model.get().replace('/', "--");
                                view! {
                                    <small style="font-weight: normal; color: #666;">
                                        "(configs/" {filename} ".toml)"
                                    </small>
                                }.into_any()
                            } else {
                                view! {
                                    <small style="font-weight: normal; color: #999;">
                                        "(none — fill in to create)"
                                    </small>
                                }.into_any()
                            }}
                        </h2>

                        <form on:submit=move |e| { e.prevent_default(); save_card_action.dispatch(()); }>
                            <table style="border-collapse: collapse; width: 100%;">
                                <tr>
                                    <td style="padding: 6px; font-weight: bold; width: 160px;">"Name"</td>
                                    <td style="padding: 6px;">
                                        <input type="text" style="width: 100%;"
                                            placeholder="e.g. Gemma 4"
                                            prop:value=move || card_name.get()
                                            on:input=move |e| card_name.set(event_target_value(&e))
                                        />
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Source (HF repo)"</td>
                                    <td style="padding: 6px;">
                                        <input type="text" style="width: 100%;"
                                            placeholder="e.g. unsloth/gemma-4-26B-A4B-it-GGUF"
                                            prop:value=move || card_source.get()
                                            on:input=move |e| card_source.set(event_target_value(&e))
                                        />
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Default context"</td>
                                    <td style="padding: 6px;">
                                        <input type="number" style="width: 100%;"
                                            placeholder="e.g. 8192"
                                            prop:value=move || card_default_ctx.get()
                                            on:input=move |e| card_default_ctx.set(event_target_value(&e))
                                        />
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold;">"Default GPU layers"</td>
                                    <td style="padding: 6px;">
                                        <input type="number" style="width: 100%;"
                                            placeholder="e.g. 999"
                                            prop:value=move || card_default_gpu.get()
                                            on:input=move |e| card_default_gpu.set(event_target_value(&e))
                                        />
                                    </td>
                                </tr>
                                <tr>
                                    <td style="padding: 6px; font-weight: bold; vertical-align: top;">"Quants (JSON)"</td>
                                    <td style="padding: 6px;">
                                        <textarea rows="10" style="width: 100%; font-family: monospace; font-size: 0.85em;"
                                            placeholder="{
  \"Q4_K_M\": { \"file\": \"model-Q4_K_M.gguf\", \"size_bytes\": 4200000000, \"context_length\": 8192 },
  \"Q6_K\":   { \"file\": \"model-Q6_K.gguf\",   \"size_bytes\": 6100000000, \"context_length\": null }
}"
                                            prop:value=move || card_quants_json.get()
                                            on:input=move |e| card_quants_json.set(event_target_value(&e))
                                        />
                                        <small>"JSON object: quant name → " <code>"{ file, size_bytes?, context_length? }"</code></small>
                                    </td>
                                </tr>
                            </table>

                            <div style="margin-top: 0.75em;">
                                <button type="submit">"Save Model Card"</button>
                            </div>

                            {move || card_status.get().map(|(ok, msg)| {
                                let color = if ok { "green" } else { "red" };
                                view! { <p style=format!("color: {}", color)>{msg}</p> }
                            })}
                        </form>

                    </div>
                }.into_any()
            }}
        </Suspense>
    }
}
