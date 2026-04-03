use leptos::prelude::*;
use leptos_router::components::A;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelEntry {
    id: String,
    backend: String,
    model: String,
    quant: Option<String>,
    enabled: bool,
    loaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsResponse {
    models: Vec<ModelEntry>,
}

#[component]
pub fn Models() -> impl IntoView {
    // Refresh trigger signal — increment to force a refetch
    let refresh = RwSignal::new(0u32);

    let models = LocalResource::new(move || async move {
        let _ = refresh.get(); // track the signal
        let resp = gloo_net::http::Request::get("/kronk/v1/models")
            .send()
            .await
            .ok()?;
        resp.json::<ModelsResponse>().await.ok()
    });

    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            let _ = gloo_net::http::Request::post(&format!("/kronk/v1/models/{}/load", id))
                .send()
                .await;
            refresh.update(|n| *n += 1);
        }
    });

    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            let _ = gloo_net::http::Request::post(&format!("/kronk/v1/models/{}/unload", id))
                .send()
                .await;
            refresh.update(|n| *n += 1);
        }
    });

    view! {
        <h1>"Models"</h1>
        <div style="margin-bottom: 1em;">
            <A href="/models/new/edit">
                <button>"+ New Model"</button>
            </A>
        </div>
        <Suspense fallback=|| view! { <p>"Loading..."</p> }>
            {move || {
                models.get().map(|guard| {
                    let result = guard.take();
                    match result {
                        Some(data) => view! {
                            <table>
                                <thead>
                                    <tr>
                                        <th>"ID"</th>
                                        <th>"Backend"</th>
                                        <th>"Model"</th>
                                        <th>"Quant"</th>
                                        <th>"Enabled"</th>
                                        <th>"Loaded"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {data.models.into_iter().map(|m| {
                                        let id_load = m.id.clone();
                                        let id_unload = m.id.clone();
                                        let id_edit = m.id.clone();
                                        view! {
                                            <tr>
                                                <td>{m.id.clone()}</td>
                                                <td>{m.backend}</td>
                                                <td>{m.model}</td>
                                                <td>{m.quant.unwrap_or_default()}</td>
                                                <td>{if m.enabled { "Yes" } else { "No" }}</td>
                                                <td>{if m.loaded { "Loaded" } else { "Unloaded" }}</td>
                                                <td style="white-space: nowrap;">
                                                    {if m.loaded {
                                                        view! {
                                                            <button on:click=move |_| { unload_action.dispatch(id_unload.clone()); }>"Unload"</button>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <button on:click=move |_| { load_action.dispatch(id_load.clone()); }>"Load"</button>
                                                        }.into_any()
                                                    }}
                                                    " "
                                                    <A href=format!("/models/{}/edit", id_edit)>
                                                        <button>"Edit"</button>
                                                    </A>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        }.into_any(),
                        None => view! { <p>"Failed to load models"</p> }.into_any(),
                    }
                })
            }}
        </Suspense>
    }
}
