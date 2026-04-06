use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigResponse {
    content: String,
}

#[component]
pub fn ConfigEditor() -> impl IntoView {
    let load_trigger = RwSignal::new(0u32);
    let editor_content = RwSignal::new(String::new());
    let status_msg = RwSignal::new(Option::<(bool, String)>::None); // (is_ok, message)

    let config = LocalResource::new(move || async move {
        let _ = load_trigger.get();
        let resp = gloo_net::http::Request::get("/api/config")
            .send()
            .await
            .ok()?;
        resp.json::<ConfigResponse>().await.ok()
    });

    // When config loads, populate the editor
    Effect::new(move |_| {
        if let Some(guard) = config.get() {
            if let Some(data) = guard.take() {
                editor_content.set(data.content);
            }
        }
    });

    let save: Action<(), (), LocalStorage> = Action::new_unsync(move |_: &()| {
        let content = editor_content.get();
        async move {
            let body = serde_json::json!({ "content": content });
            match gloo_net::http::Request::post("/api/config")
                .json(&body)
                .map(|r| r.send())
            {
                Ok(fut) => match fut.await {
                    Ok(resp) => {
                        if resp.status() == 200 {
                            status_msg.set(Some((true, "Config saved successfully.".to_string())));
                        } else {
                            let msg = resp
                                .text()
                                .await
                                .unwrap_or_else(|_| "Unknown error".to_string());
                            status_msg.set(Some((false, format!("Error: {}", msg))));
                        }
                    }
                    Err(e) => {
                        status_msg.set(Some((false, format!("Request failed: {}", e))));
                    }
                },
                Err(e) => {
                    status_msg.set(Some((false, format!("Failed to build request: {}", e))));
                }
            }
        }
    });

    view! {
        <div class="page-header">
            <h1>"Config"</h1>
            <div class="form-actions">
                <button
                    class="btn btn-primary"
                    on:click=move |_| { save.dispatch(()); }
                >
                    "Save"
                </button>
                <button
                    class="btn btn-secondary"
                    on:click=move |_| { load_trigger.update(|n| *n += 1); }
                >
                    "↺ Reload"
                </button>
            </div>
        </div>

        <Suspense fallback=|| view! {
            <div class="spinner-container">
                <span class="spinner"></span>
                <span class="text-muted">"Loading config..."</span>
            </div>
        }>
            {move || {
                // Just trigger the resource read so Suspense knows when loading is done
                let _ = config.get();
                view! {
                    <div class="form-card card">
                        <div class="form-card__header">
                            <h2 class="form-card__title">"koji.toml"</h2>
                            <p class="form-card__desc text-muted">
                                "Edit the raw TOML configuration. Changes take effect after saving and reloading the service."
                            </p>
                        </div>

                        <div class="form-group">
                            <textarea
                                class="code-editor"
                                rows="30"
                                prop:value=move || editor_content.get()
                                on:input=move |e| editor_content.set(event_target_value(&e))
                            />
                        </div>

                        {move || status_msg.get().map(|(ok, msg)| {
                            let cls = if ok { "alert alert--success mt-2" } else { "alert alert--error mt-2" };
                            let icon = if ok { "✓" } else { "✕" };
                            view! {
                                <div class=cls>
                                    <span class="alert__icon">{icon}</span>
                                    <span>{msg}</span>
                                </div>
                            }
                        })}
                    </div>
                }.into_any()
            }}
        </Suspense>
    }
}
