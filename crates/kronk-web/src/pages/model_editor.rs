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
    args: String,
    profile: String,
    enabled: bool,
    context_length: String,
    port: String,
}

/// One row in the quants editor table.
#[derive(Debug, Clone, Default)]
struct QuantRow {
    name: String,
    file: String,
    size_bytes: String,
    context_length: String,
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

/// Convert the card's quants map into sorted rows for the editor.
fn quants_to_rows(quants: &HashMap<String, QuantInfo>) -> Vec<QuantRow> {
    let mut rows: Vec<QuantRow> = quants
        .iter()
        .map(|(name, q)| QuantRow {
            name: name.clone(),
            file: q.file.clone(),
            size_bytes: q.size_bytes.map(|v| v.to_string()).unwrap_or_default(),
            context_length: q.context_length.map(|v| v.to_string()).unwrap_or_default(),
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// Collect the quant rows into the JSON object the API expects.
fn rows_to_quants_json(rows: &[QuantRow]) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = rows
        .iter()
        .filter(|r| !r.name.trim().is_empty())
        .map(|r| {
            let val = serde_json::json!({
                "file": r.file,
                "size_bytes": r.size_bytes.trim().parse::<u64>().ok(),
                "context_length": r.context_length.trim().parse::<u32>().ok(),
            });
            (r.name.trim().to_string(), val)
        })
        .collect();
    serde_json::Value::Object(map)
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

async fn save_card(
    model_id: String,
    name: String,
    source: String,
    default_ctx: String,
    default_gpu: String,
    quant_rows: Vec<QuantRow>,
) -> Result<(), String> {
    let body = serde_json::json!({
        "name": name,
        "source": source,
        "default_context_length": default_ctx.parse::<u32>().ok(),
        "default_gpu_layers": default_gpu.parse::<u32>().ok(),
        "quants": rows_to_quants_json(&quant_rows),
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

    // ── Model config signals ──────────────────────────────────────────────────
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

    // ── Card signals ──────────────────────────────────────────────────────────
    let card_name = RwSignal::new(String::new());
    let card_source = RwSignal::new(String::new());
    let card_default_ctx = RwSignal::new(String::new());
    let card_default_gpu = RwSignal::new(String::new());
    // Each quant row is its own RwSignal so individual fields are reactive
    let quant_rows: RwSignal<Vec<RwSignal<QuantRow>>> = RwSignal::new(vec![]);
    let has_card = RwSignal::new(false);

    // ── Status ────────────────────────────────────────────────────────────────
    let model_status = RwSignal::new(Option::<(bool, String)>::None);
    let card_status = RwSignal::new(Option::<(bool, String)>::None);
    let deleted = RwSignal::new(false);

    // Populate signals when the resource loads
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
                    card_name.set(card.model.name.clone());
                    card_source.set(card.model.source.clone());
                    card_default_ctx.set(
                        card.model
                            .default_context_length
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                    );
                    card_default_gpu.set(
                        card.model
                            .default_gpu_layers
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                    );
                    let rows = quants_to_rows(&card.quants)
                        .into_iter()
                        .map(RwSignal::new)
                        .collect();
                    quant_rows.set(rows);
                } else {
                    has_card.set(false);
                    card_source.set(f.model);
                    quant_rows.set(vec![]);
                }
            }
        }
    });

    // ── Actions ───────────────────────────────────────────────────────────────

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
            // Snapshot all row signals into plain values
            let rows: Vec<QuantRow> = quant_rows.get().iter().map(|s| s.get()).collect();
            match save_card(
                id,
                card_name.get(),
                card_source.get(),
                card_default_ctx.get(),
                card_default_gpu.get(),
                rows,
            )
            .await
            {
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

    // ── View ──────────────────────────────────────────────────────────────────

    view! {
        <div class="page-header">
            <h1>{move || if is_new() { "New Model".to_string() } else { format!("Edit: {}", model_id()) }}</h1>
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
                    <div class="editor-layout">

                        // ── Model Config ──────────────────────────────────────
                        <div class="form-card--wide card">
                            <div class="form-card__header">
                                <h2 class="form-card__title">"Model Config"</h2>
                                <p class="form-card__desc text-muted">
                                    "Configure backend, model source, and runtime parameters."
                                </p>
                            </div>

                            <form on:submit=move |e| { e.prevent_default(); save_model_action.dispatch(()); }>
                                <div class="form-grid mb-3">
                                    <label class="form-label" for="field-id">"ID"</label>
                                    <input
                                        id="field-id"
                                        class="form-input"
                                        type="text"
                                        placeholder="e.g. my-model"
                                        prop:value=move || form_id.get()
                                        prop:disabled=move || !is_new()
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
                                    <input
                                        id="field-quant"
                                        class="form-input"
                                        type="text"
                                        placeholder="e.g. Q4_K_M"
                                        prop:value=move || form_quant.get()
                                        on:input=move |e| form_quant.set(event_target_value(&e))
                                    />

                                    <label class="form-label" for="field-profile">"Profile"</label>
                                    <select
                                        id="field-profile"
                                        class="form-select"
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

                                    <label class="form-label" for="field-args">"Extra args"</label>
                                    <div>
                                        <textarea
                                            id="field-args"
                                            class="form-textarea"
                                            rows="6"
                                            placeholder="One flag per line, e.g.:\n-ctk\nq4_0"
                                            prop:value=move || form_args.get()
                                            on:input=move |e| form_args.set(event_target_value(&e))
                                        />
                                        <span class="form-hint">"One argument per line (same as TOML args array)"</span>
                                    </div>
                                </div>

                                <hr class="section-divider mb-3" />

                                <div class="form-actions">
                                    <button type="submit" class="btn btn-primary">"Save Model Config"</button>
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

                        // ── Model Card ────────────────────────────────────────
                        <div class="form-card--wide card">
                            <div class="form-card__header">
                                <h2 class="form-card__title">
                                    "Model Card"
                                    {move || if has_card.get() {
                                        let filename = form_model.get().replace('/', "--");
                                        view! {
                                            <span class="text-muted card-subtitle">
                                                "(configs/" {filename} ".toml)"
                                            </span>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <span class="text-muted card-subtitle">
                                                "(none — fill in to create)"
                                            </span>
                                        }.into_any()
                                    }}
                                </h2>
                                <p class="form-card__desc text-muted">
                                    "Store display name, source repo, and available quantisations."
                                </p>
                            </div>

                            <form on:submit=move |e| { e.prevent_default(); save_card_action.dispatch(()); }>
                                <div class="form-grid mb-3">
                                    <label class="form-label" for="card-name">"Name"</label>
                                    <input
                                        id="card-name"
                                        class="form-input"
                                        type="text"
                                        placeholder="e.g. Gemma 4"
                                        prop:value=move || card_name.get()
                                        on:input=move |e| card_name.set(event_target_value(&e))
                                    />

                                    <label class="form-label" for="card-source">"Source (HF repo)"</label>
                                    <input
                                        id="card-source"
                                        class="form-input"
                                        type="text"
                                        placeholder="e.g. unsloth/gemma-4-26B-A4B-it-GGUF"
                                        prop:value=move || card_source.get()
                                        on:input=move |e| card_source.set(event_target_value(&e))
                                    />

                                    <label class="form-label" for="card-ctx">"Default context length"</label>
                                    <input
                                        id="card-ctx"
                                        class="form-input"
                                        type="number"
                                        placeholder="e.g. 8192"
                                        prop:value=move || card_default_ctx.get()
                                        on:input=move |e| card_default_ctx.set(event_target_value(&e))
                                    />

                                    <label class="form-label" for="card-gpu">"Default GPU layers"</label>
                                    <input
                                        id="card-gpu"
                                        class="form-input"
                                        type="number"
                                        placeholder="e.g. 999"
                                        prop:value=move || card_default_gpu.get()
                                        on:input=move |e| card_default_gpu.set(event_target_value(&e))
                                    />
                                </div>

                                // ── Quants table ──────────────────────────────
                                <div class="form-group">
                                    <label class="form-label">"Quants"</label>
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
                                                each=move || quant_rows.get().into_iter().enumerate()
                                                key=|(i, _)| *i
                                                children=move |(i, row_signal)| {
                                                    view! {
                                                        <tr>
                                                            <td>
                                                                <input
                                                                    class="form-input"
                                                                    type="text"
                                                                    style="width:8em;"
                                                                    placeholder="Q4_K_M"
                                                                    prop:value=move || row_signal.get().name.clone()
                                                                    on:input=move |e| row_signal.update(|r| r.name = event_target_value(&e))
                                                                />
                                                            </td>
                                                            <td>
                                                                <input
                                                                    class="form-input"
                                                                    type="text"
                                                                    placeholder="model-Q4_K_M.gguf"
                                                                    prop:value=move || row_signal.get().file.clone()
                                                                    on:input=move |e| row_signal.update(|r| r.file = event_target_value(&e))
                                                                />
                                                            </td>
                                                            <td>
                                                                <input
                                                                    class="form-input"
                                                                    type="number"
                                                                    style="width:9em;"
                                                                    placeholder="optional"
                                                                    prop:value=move || row_signal.get().size_bytes.clone()
                                                                    on:input=move |e| row_signal.update(|r| r.size_bytes = event_target_value(&e))
                                                                />
                                                            </td>
                                                            <td>
                                                                <input
                                                                    class="form-input"
                                                                    type="number"
                                                                    style="width:7em;"
                                                                    placeholder="optional"
                                                                    prop:value=move || row_signal.get().context_length.clone()
                                                                    on:input=move |e| row_signal.update(|r| r.context_length = event_target_value(&e))
                                                                />
                                                            </td>
                                                            <td>
                                                                <button
                                                                    type="button"
                                                                    class="btn btn-danger btn-sm"
                                                                    on:click=move |_| {
                                                                        quant_rows.update(|rows| { rows.remove(i); });
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
                                        <button
                                            type="button"
                                            class="btn btn-secondary btn-sm"
                                            on:click=move |_| {
                                                quant_rows.update(|rows| rows.push(RwSignal::new(QuantRow::default())));
                                            }
                                        >"+ Add Quant"</button>
                                    </div>
                                </div>

                                <hr class="section-divider mb-3" />

                                <div class="form-actions">
                                    <button type="submit" class="btn btn-primary">"Save Model Card"</button>
                                </div>

                                {move || card_status.get().map(|(ok, msg)| {
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

                    </div>
                }.into_any()
            }}
        </Suspense>
    }
}
