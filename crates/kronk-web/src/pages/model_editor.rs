use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelListResponse {
    models: Vec<ModelDetail>,
    backends: Vec<String>,
}

/// Fetch a single model for editing. `id` is "new" for the create form.
async fn fetch_model(id: String) -> Option<ModelDetail> {
    if id == "new" {
        // Fetch backends list from /api/models to populate dropdown
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

#[component]
pub fn ModelEditor() -> impl IntoView {
    let params = use_params_map();
    let model_id = move || params.get().get("id").unwrap_or_default();
    let is_new = move || model_id() == "new";

    let detail = LocalResource::new(move || {
        let id = model_id();
        async move { fetch_model(id).await }
    });

    // Form signals
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
    let status_msg = RwSignal::new(Option::<(bool, String)>::None);
    let deleted = RwSignal::new(false);

    // Populate form when resource loads
    Effect::new(move |_| {
        if let Some(guard) = detail.get() {
            if let Some(d) = guard.take() {
                backends.set(d.backends.clone());
                let f = detail_to_form(&d);
                form_id.set(f.id);
                form_backend.set(f.backend);
                form_model.set(f.model);
                form_quant.set(f.quant);
                form_args.set(f.args);
                form_profile.set(f.profile);
                form_enabled.set(f.enabled);
                form_context_length.set(f.context_length);
                form_port.set(f.port);
            }
        }
    });

    let save: Action<(), (), LocalStorage> = Action::new_unsync(move |_: &()| async move {
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
            Ok(()) => status_msg.set(Some((true, "Saved successfully.".into()))),
            Err(e) => status_msg.set(Some((false, format!("Error: {}", e)))),
        }
    });

    let delete: Action<(), (), LocalStorage> = Action::new_unsync(move |_: &()| async move {
        let id = form_id.get();
        match delete_model_api(id).await {
            Ok(()) => deleted.set(true),
            Err(e) => status_msg.set(Some((false, format!("Delete failed: {}", e)))),
        }
    });

    view! {
        <h1>{move || if is_new() { "New Model".to_string() } else { format!("Edit Model: {}", model_id()) }}</h1>

        // Redirect hint after delete
        {move || deleted.get().then(|| view! {
            <p style="color: green">
                "Model deleted. "
                <A href="/models">"Back to Models"</A>
            </p>
        })}

        <Suspense fallback=|| view! { <p>"Loading..."</p> }>
            {move || {
                let _ = detail.get(); // trigger suspense tracking
                view! {
                    <form on:submit=move |e| { e.prevent_default(); save.dispatch(()); }>
                        <table style="border-collapse: collapse; width: 100%;">

                            // ID — only editable when creating new
                            <tr>
                                <td style="padding: 6px; font-weight: bold;">"ID"</td>
                                <td style="padding: 6px;">
                                    <input
                                        type="text"
                                        style="width: 100%;"
                                        placeholder="e.g. my-model"
                                        prop:value=move || form_id.get()
                                        prop:disabled=move || !is_new()
                                        on:input=move |e| form_id.set(event_target_value(&e))
                                    />
                                </td>
                            </tr>

                            // Backend dropdown
                            <tr>
                                <td style="padding: 6px; font-weight: bold;">"Backend"</td>
                                <td style="padding: 6px;">
                                    <select
                                        style="width: 100%;"
                                        on:change=move |e| form_backend.set(event_target_value(&e))
                                    >
                                        {move || backends.get().into_iter().map(|b| {
                                            let selected = b == form_backend.get();
                                            let b2 = b.clone();
                                            view! {
                                                <option value=b.clone() selected=selected>{b2}</option>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </select>
                                </td>
                            </tr>

                            // Model (HF repo)
                            <tr>
                                <td style="padding: 6px; font-weight: bold;">"Model (HF repo)"</td>
                                <td style="padding: 6px;">
                                    <input
                                        type="text"
                                        style="width: 100%;"
                                        placeholder="e.g. unsloth/gemma-4-26B-A4B-it-GGUF"
                                        prop:value=move || form_model.get()
                                        on:input=move |e| form_model.set(event_target_value(&e))
                                    />
                                </td>
                            </tr>

                            // Quant
                            <tr>
                                <td style="padding: 6px; font-weight: bold;">"Quant"</td>
                                <td style="padding: 6px;">
                                    <input
                                        type="text"
                                        style="width: 100%;"
                                        placeholder="e.g. Q4_K_M"
                                        prop:value=move || form_quant.get()
                                        on:input=move |e| form_quant.set(event_target_value(&e))
                                    />
                                </td>
                            </tr>

                            // Profile dropdown
                            <tr>
                                <td style="padding: 6px; font-weight: bold;">"Profile"</td>
                                <td style="padding: 6px;">
                                    <select
                                        style="width: 100%;"
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

                            // Context length
                            <tr>
                                <td style="padding: 6px; font-weight: bold;">"Context length"</td>
                                <td style="padding: 6px;">
                                    <input
                                        type="number"
                                        style="width: 100%;"
                                        placeholder="e.g. 8192 (leave blank for default)"
                                        prop:value=move || form_context_length.get()
                                        on:input=move |e| form_context_length.set(event_target_value(&e))
                                    />
                                </td>
                            </tr>

                            // Port
                            <tr>
                                <td style="padding: 6px; font-weight: bold;">"Port override"</td>
                                <td style="padding: 6px;">
                                    <input
                                        type="number"
                                        style="width: 100%;"
                                        placeholder="leave blank for default"
                                        prop:value=move || form_port.get()
                                        on:input=move |e| form_port.set(event_target_value(&e))
                                    />
                                </td>
                            </tr>

                            // Enabled checkbox
                            <tr>
                                <td style="padding: 6px; font-weight: bold;">"Enabled"</td>
                                <td style="padding: 6px;">
                                    <input
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
                                </td>
                            </tr>

                            // Extra args (one per line)
                            <tr>
                                <td style="padding: 6px; font-weight: bold; vertical-align: top;">"Extra args"</td>
                                <td style="padding: 6px;">
                                    <textarea
                                        rows="8"
                                        style="width: 100%; font-family: monospace; font-size: 0.85em;"
                                        placeholder="One flag per line, e.g.:\n-ctk\nq4_0\n-ngl\n999"
                                        prop:value=move || form_args.get()
                                        on:input=move |e| form_args.set(event_target_value(&e))
                                    />
                                    <small>"One argument per line (same as TOML args array)"</small>
                                </td>
                            </tr>

                        </table>

                        <div style="margin-top: 1em; display: flex; gap: 0.5em; align-items: center;">
                            <button type="submit">"Save"</button>
                            <A href="/models">
                                <button type="button">"Cancel"</button>
                            </A>
                            {move || (!is_new()).then(|| view! {
                                <button
                                    type="button"
                                    style="margin-left: auto; background: #c0392b; color: white; border: none; padding: 0.4em 1em; cursor: pointer;"
                                    on:click=move |_| {
                                        // Use window.confirm for deletion prompt
                                        let confirmed = web_sys::window()
                                            .and_then(|w| w.confirm_with_message("Delete this model? This cannot be undone.").ok())
                                            .unwrap_or(false);
                                        if confirmed {
                                            delete.dispatch(());
                                        }
                                    }
                                >"Delete Model"</button>
                            })}
                        </div>

                        {move || status_msg.get().map(|(ok, msg)| {
                            let color = if ok { "green" } else { "red" };
                            view! { <p style=format!("color: {}", color)>{msg}</p> }
                        })}
                    </form>
                }.into_any()
            }}
        </Suspense>
    }
}
