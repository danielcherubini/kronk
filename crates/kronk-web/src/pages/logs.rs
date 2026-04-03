use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogsResponse {
    lines: Vec<String>,
}

#[component]
pub fn Logs() -> impl IntoView {
    let refresh = RwSignal::new(0u32);

    let logs = LocalResource::new(move || async move {
        let _ = refresh.get(); // track the signal
        let resp = gloo_net::http::Request::get("/api/logs")
            .send()
            .await
            .ok()?;
        resp.json::<LogsResponse>().await.ok()
    });

    view! {
        <h1>"Log Viewer"</h1>
        <button on:click=move |_| { refresh.update(|n| *n += 1); }>"Refresh"</button>
        <Suspense fallback=|| view! { <p>"Loading..."</p> }>
            {move || {
                logs.get().map(|guard| {
                    let result = guard.take();
                    match result {
                        Some(data) => {
                            let text = data.lines.join("\n");
                            view! {
                                <pre style="overflow: auto; max-height: 600px; background: #1e1e1e; color: #d4d4d4; padding: 1em; font-size: 0.85em;">
                                    {text}
                                </pre>
                            }.into_any()
                        }
                        None => view! { <p>"Failed to load logs (is logs_dir configured?)"</p> }.into_any(),
                    }
                })
            }}
        </Suspense>
    }
}
