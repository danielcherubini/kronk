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
        <h1>"Config Editor"</h1>
        <Suspense fallback=|| view! { <p>"Loading..."</p> }>
            {move || {
                // Just trigger the resource read so Suspense knows when loading is done
                let _ = config.get();
                view! {
                    <div>
                        <textarea
                            rows="30"
                            style="width: 100%; font-family: monospace; font-size: 0.9em;"
                            prop:value=move || editor_content.get()
                            on:input=move |e| editor_content.set(event_target_value(&e))
                        />
                        <div>
                            <button on:click=move |_| { save.dispatch(()); }>"Save"</button>
                            <button on:click=move |_| { load_trigger.update(|n| *n += 1); } style="margin-left: 0.5em;">"Reload"</button>
                        </div>
                        {move || status_msg.get().map(|(ok, msg)| {
                            let color = if ok { "green" } else { "red" };
                            view! { <p style=format!("color: {}", color)>{msg}</p> }
                        })}
                    </div>
                }.into_any()
            }}
        </Suspense>
    }
}
