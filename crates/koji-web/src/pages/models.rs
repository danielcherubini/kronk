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

fn partition_models_by_loaded(models: Vec<ModelEntry>) -> (Vec<ModelEntry>, Vec<ModelEntry>) {
    let (mut loaded, mut unloaded): (Vec<_>, Vec<_>) = models.into_iter().partition(|m| m.loaded);
    loaded.sort_by(|a, b| a.id.cmp(&b.id));
    unloaded.sort_by(|a, b| a.id.cmp(&b.id));
    (loaded, unloaded)
}

#[component]
pub fn Models() -> impl IntoView {
    // Refresh trigger signal — increment to force a refetch
    let refresh = RwSignal::new(0u32);

    let models = LocalResource::new(move || async move {
        let _ = refresh.get(); // track the signal
        let resp = gloo_net::http::Request::get("/koji/v1/models")
            .send()
            .await
            .ok()?;
        resp.json::<ModelsResponse>().await.ok()
    });

    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            let _ = gloo_net::http::Request::post(&format!("/koji/v1/models/{}/load", id))
                .send()
                .await;
            refresh.update(|n| *n += 1);
        }
    });

    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            let _ = gloo_net::http::Request::post(&format!("/koji/v1/models/{}/unload", id))
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
                        Some(data) => {
                            let (loaded, unloaded) = partition_models_by_loaded(data.models);
                            view! {
                                <div>
                                    {if !loaded.is_empty() {
                                        view! {
                                            <div class="model-section">
                                                <h2 class="model-section__title">"Loaded Models"</h2>
                                                <div class="models-grid">
                                                    {loaded.into_iter().map(|m| {
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
                                                                                on:click=move |_| {
                                                                                    unload_action.dispatch(id_unload.clone());
                                                                                    refresh.update(|n| *n += 1);
                                                                                }
                                                                            >
                                                                                "Unload"
                                                                            </button>
                                                                        }.into_any()
                                                                    } else {
                                                                        view! {
                                                                            <button
                                                                                class="btn btn-success btn-sm"
                                                                                on:click=move |_| {
                                                                                    load_action.dispatch(id_load.clone());
                                                                                    refresh.update(|n| *n += 1);
                                                                                }
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
                                            </div>
                                        }.into_any()
                                    } else {
                                        let _: () = view! { <></> };
                                        ().into_any()
                                    }}
                                    {if !unloaded.is_empty() {
                                        view! {
                                            <div class="model-section">
                                                <h2 class="model-section__title">"Unloaded Models"</h2>
                                                <div class="models-grid">
                                                    {unloaded.into_iter().map(|m| {
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
                                                                                on:click=move |_| {
                                                                                    unload_action.dispatch(id_unload.clone());
                                                                                    refresh.update(|n| *n += 1);
                                                                                }
                                                                            >
                                                                                "Unload"
                                                                            </button>
                                                                        }.into_any()
                                                                    } else {
                                                                        view! {
                                                                            <button
                                                                                class="btn btn-success btn-sm"
                                                                                on:click=move |_| {
                                                                                    load_action.dispatch(id_load.clone());
                                                                                    refresh.update(|n| *n += 1);
                                                                                }
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
                                            </div>
                                        }.into_any()
                                    } else {
                                        let _: () = view! { <></> };
                                        ().into_any()
                                    }}
                                </div>
                            }.into_any()
                        },
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

#[cfg(test)]
mod tests {
    use super::*;

    // partition_models_by_loaded is defined above

    #[test]
    fn test_all_loaded_returns_n_zero() {
        let models = vec![
            ModelEntry {
                id: "model-1".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama2".to_string(),
                quant: None,
                enabled: true,
                loaded: true,
            },
            ModelEntry {
                id: "model-2".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama3".to_string(),
                quant: None,
                enabled: true,
                loaded: true,
            },
        ];
        let (loaded, unloaded) = partition_models_by_loaded(models);
        assert_eq!(loaded.len(), 2);
        assert_eq!(unloaded.len(), 0);
    }

    #[test]
    fn test_all_unloaded_returns_zero_n() {
        let models = vec![
            ModelEntry {
                id: "model-1".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama2".to_string(),
                quant: None,
                enabled: true,
                loaded: false,
            },
            ModelEntry {
                id: "model-2".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama3".to_string(),
                quant: None,
                enabled: true,
                loaded: false,
            },
        ];
        let (loaded, unloaded) = partition_models_by_loaded(models);
        assert_eq!(loaded.len(), 0);
        assert_eq!(unloaded.len(), 2);
    }

    #[test]
    fn test_mixed_returns_correct_split() {
        let models = vec![
            ModelEntry {
                id: "model-1".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama2".to_string(),
                quant: None,
                enabled: true,
                loaded: true,
            },
            ModelEntry {
                id: "model-2".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama3".to_string(),
                quant: None,
                enabled: true,
                loaded: false,
            },
            ModelEntry {
                id: "model-3".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama4".to_string(),
                quant: None,
                enabled: true,
                loaded: true,
            },
        ];
        let (loaded, unloaded) = partition_models_by_loaded(models);
        assert_eq!(loaded.len(), 2);
        assert_eq!(unloaded.len(), 1);
    }

    #[test]
    fn test_empty_returns_zero_zero() {
        let models: Vec<ModelEntry> = vec![];
        let (loaded, unloaded) = partition_models_by_loaded(models);
        assert_eq!(loaded.len(), 0);
        assert_eq!(unloaded.len(), 0);
    }

    #[test]
    fn test_sorts_both_partitions_by_id() {
        let models = vec![
            ModelEntry {
                id: "model-2".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama3".to_string(),
                quant: None,
                enabled: true,
                loaded: true,
            },
            ModelEntry {
                id: "model-1".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama2".to_string(),
                quant: None,
                enabled: true,
                loaded: true,
            },
            ModelEntry {
                id: "model-4".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama5".to_string(),
                quant: None,
                enabled: true,
                loaded: false,
            },
            ModelEntry {
                id: "model-3".to_string(),
                backend: "llama.cpp".to_string(),
                model: "llama4".to_string(),
                quant: None,
                enabled: true,
                loaded: false,
            },
        ];
        let (loaded, unloaded) = partition_models_by_loaded(models);
        // Check loaded partition is sorted by id
        assert_eq!(loaded[0].id, "model-1");
        assert_eq!(loaded[1].id, "model-2");
        // Check unloaded partition is sorted by id
        assert_eq!(unloaded[0].id, "model-3");
        assert_eq!(unloaded[1].id, "model-4");
    }
}
