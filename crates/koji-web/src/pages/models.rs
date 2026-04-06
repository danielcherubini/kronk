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
        <div class="page-header">
            <h1>"Models"</h1>
            <A href="/models/new/edit">
                <button class="btn btn-primary">"+ New Model"</button>
            </A>
        </div>
        <Suspense fallback=|| view! {
            <div class="card card--centered">
                <span class="spinner">"Loading models..."</span>
            </div>
        }>
            {move || {
                models.get().map(|guard| {
                    let result = guard.take();
                    match result {
                        Some(data) if data.models.is_empty() => view! {
                            <div class="card card--centered">
                                <p class="text-muted">"No models configured yet."</p>
                                <a href="/pull"><button class="btn btn-primary mt-2">"Pull a Model"</button></a>
                            </div>
                        }.into_any(),
                        Some(data) => view! {
                            <div class="models-grid">
                                {data.models.into_iter().map(|m| {
                                    let id_load = m.id.clone();
                                    let id_unload = m.id.clone();
                                    let id_edit = m.id.clone();
                                    let enabled_class = if m.enabled { "badge badge-success" } else { "badge badge-warning" };
                                    let loaded_class = if m.loaded { "badge badge-success" } else { "badge badge-muted" };
                                    view! {
                                        <div class="model-card card">
                                            <div class="model-card__header">
                                                <span class="model-card__id text-mono">{m.id.clone()}</span>
                                                <div class="model-card__badges">
                                                    <span class=enabled_class>
                                                        {if m.enabled { "Enabled" } else { "Disabled" }}
                                                    </span>
                                    <span class=loaded_class>
                                        {if m.loaded { "Loaded" } else { "Idle" }}
                                    </span>
                                                </div>
                                            </div>
                                            <div class="model-card__body">
                                                <div class="model-card__field">
                                                    <span class="model-card__label">"Backend"</span>
                                                    <span class="model-card__value text-mono">{m.backend}</span>
                                                </div>
                                                <div class="model-card__field">
                                                    <span class="model-card__label">"Model"</span>
                                                    <span class="model-card__value text-mono">{m.model}</span>
                                                </div>
                                                {m.quant.map(|q| view! {
                                                    <div class="model-card__field">
                                                        <span class="model-card__label">"Quant"</span>
                                                        <span class="model-card__value text-mono">{q}</span>
                                                    </div>
                                                })}
                                            </div>
                                            <div class="model-card__actions">
                                                {if m.loaded {
                                                    view! {
                                                        <button
                                                            class="btn btn-danger btn-sm"
                                                            on:click=move |_| { unload_action.dispatch(id_unload.clone()); }
                                                        >
                                                            "Unload"
                                                        </button>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <button
                                                            class="btn btn-success btn-sm"
                                                            on:click=move |_| { load_action.dispatch(id_load.clone()); }
                                                        >
                                                            "Load"
                                                        </button>
                                                    }.into_any()
                                                }}
                                                <A href=format!("/models/{}/edit", id_edit)>
                                                    <button class="btn btn-secondary btn-sm">"Edit"</button>
                                                </A>
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any(),
                        None => view! {
                            <div class="card">
                                <p class="text-error">"Failed to load models."</p>
                            </div>
                        }.into_any(),
                    }
                })
            }}
        </Suspense>
    }
}
