use leptos::prelude::*;
use leptos_router::components::A;
use serde::{Deserialize, Serialize};

use crate::components::modal::Modal;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelEntry {
    id: String,
    backend: String,
    model: Option<String>,
    quant: Option<String>,
    enabled: bool,
    loaded: bool,
    #[serde(default)]
    api_name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
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

fn rw_signal_to_signal<T: Clone + Send + Sync + 'static>(sig: RwSignal<T>) -> Signal<T> {
    let (read, _) = sig.split();
    read.into()
}

/// Returns the preferred display name for a model, preferring `display_name`,
/// then `api_name`, falling back to the model `id` otherwise.
fn model_display_name(m: &ModelEntry) -> String {
    m.display_name
        .as_deref()
        .or(m.api_name.as_deref())
        .unwrap_or(m.id.as_str())
        .to_string()
}

#[component]
pub fn Models() -> impl IntoView {
    // Refresh trigger signal — increment to force a refetch
    let refresh = RwSignal::new(0u32);
    let pull_modal_open = RwSignal::new(false);

    // Global "Check all for updates" status
    let check_all_busy = RwSignal::new(false);
    let check_all_status = RwSignal::new(Option::<(bool, String)>::None);

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

    // Fire POST /api/models/:id/refresh for every model sequentially. Safe to
    // run without progress streaming because refresh is a pair of small HTTP
    // calls per model (no downloads, no hashing).
    let check_all_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            check_all_busy.set(true);
            check_all_status.set(None);
            // Fetch the list directly from the backend that exposes `id`s with
            // DB metadata so we iterate over the same set the editor operates on.
            let resp = match gloo_net::http::Request::get("/api/models").send().await {
                Ok(r) => r,
                Err(e) => {
                    check_all_status.set(Some((false, format!("Failed to list models: {}", e))));
                    check_all_busy.set(false);
                    return;
                }
            };
            // Surface non-2xx HTTP responses instead of silently falling
            // through to an empty list, which would report "Refreshed 0/0
            // models successfully" on a real server error.
            if !resp.ok() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                check_all_status.set(Some((
                    false,
                    format!("Failed to list models: HTTP {} {}", status, body),
                )));
                check_all_busy.set(false);
                return;
            }
            let list = match resp.json::<serde_json::Value>().await {
                Ok(v) => v,
                Err(e) => {
                    check_all_status
                        .set(Some((false, format!("Failed to parse models list: {}", e))));
                    check_all_busy.set(false);
                    return;
                }
            };
            let ids: Vec<String> = list
                .get("models")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let total = ids.len();
            let mut ok_count = 0usize;
            let mut failed = Vec::<String>::new();
            for id in ids {
                // Percent-encode the id so values containing `/`, spaces or
                // other reserved characters route correctly to the backend.
                let encoded_id = urlencoding::encode(&id);
                let url = format!("/api/models/{}/refresh", encoded_id);
                match gloo_net::http::Request::post(&url).send().await {
                    Ok(r) if r.status() == 200 => ok_count += 1,
                    Ok(r) => {
                        let text = r.text().await.unwrap_or_default();
                        failed.push(format!("{}: {}", id, text));
                    }
                    Err(e) => failed.push(format!("{}: {}", id, e)),
                }
            }

            if failed.is_empty() {
                check_all_status.set(Some((
                    true,
                    format!("Refreshed {}/{} models successfully.", ok_count, total),
                )));
            } else {
                check_all_status.set(Some((
                    false,
                    format!(
                        "Refreshed {}/{} models. Failures: {}",
                        ok_count,
                        total,
                        failed.join("; ")
                    ),
                )));
            }
            check_all_busy.set(false);
            refresh.update(|n| *n += 1);
        });

    view! {
        <div class="page-header">
            <h1>"Models"</h1>
            <div class="page-header__actions">
                <button
                    class="btn btn-secondary"
                    prop:disabled=move || check_all_busy.get()
                    on:click=move |_| { check_all_action.dispatch(()); }
                    title="Check HuggingFace for updated metadata on every model"
                >
                    {move || if check_all_busy.get() { "Checking..." } else { "Check all for updates" }}
                </button>
                <button class="btn btn-primary" on:click=move |_| pull_modal_open.set(true)>
                    "Pull Model"
                </button>
            </div>
        </div>
        {move || check_all_status.get().map(|(ok, msg)| {
            let cls = if ok { "alert alert--success" } else { "alert alert--error" };
            view! { <div class=cls>{msg}</div> }
        })}
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
                                <button class="btn btn-primary mt-2" on:click=move |_| pull_modal_open.set(true)>
                                    "Pull a Model"
                                </button>
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
                                                                    <span class="model-card__id text-mono">{model_display_name(&m)}</span>
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
                                                                    {m.model.map(|repo| view! {
                                                                        <div class="model-card__field">
                                                                            <span class="model-card__label">"Model"</span>
                                                                            <span class="model-card__value text-mono">{repo}</span>
                                                                        </div>
                                                                    })}
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
                                                                    <span class="model-card__id text-mono">{model_display_name(&m)}</span>
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
                                                                    {m.model.map(|repo| view! {
                                                                        <div class="model-card__field">
                                                                            <span class="model-card__label">"Model"</span>
                                                                            <span class="model-card__value text-mono">{repo}</span>
                                                                        </div>
                                                                    })}
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
        <Modal
            open=rw_signal_to_signal(pull_modal_open)
            on_close=Callback::new(move |_| pull_modal_open.set(false))
            title="Pull Model".to_string()
        >
            <PullQuantWizard
                initial_repo=Signal::derive(String::new)
                is_open=rw_signal_to_signal(pull_modal_open)
                on_complete=Callback::new(move |_completed: Vec<CompletedQuant>| {
                    pull_modal_open.set(false);
                    refresh.update(|n| *n += 1);
                })
                on_close=Callback::new(move |_| pull_modal_open.set(false))
            />
        </Modal>
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
                model: Some("llama2".to_string()),
                quant: None,
                enabled: true,
                loaded: true,
                api_name: None,
                display_name: None,
            },
            ModelEntry {
                id: "model-2".to_string(),
                backend: "llama.cpp".to_string(),
                model: Some("llama3".to_string()),
                quant: None,
                enabled: true,
                loaded: true,
                api_name: None,
                display_name: None,
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
                model: Some("llama2".to_string()),
                quant: None,
                enabled: true,
                loaded: false,
                api_name: None,
                display_name: None,
            },
            ModelEntry {
                id: "model-2".to_string(),
                backend: "llama.cpp".to_string(),
                model: Some("llama3".to_string()),
                quant: None,
                enabled: true,
                loaded: false,
                api_name: None,
                display_name: None,
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
                model: Some("llama2".to_string()),
                quant: None,
                enabled: true,
                loaded: true,
                api_name: None,
                display_name: None,
            },
            ModelEntry {
                id: "model-2".to_string(),
                backend: "llama.cpp".to_string(),
                model: Some("llama3".to_string()),
                quant: None,
                enabled: true,
                loaded: false,
                api_name: None,
                display_name: None,
            },
            ModelEntry {
                id: "model-3".to_string(),
                backend: "llama.cpp".to_string(),
                model: Some("llama4".to_string()),
                quant: None,
                enabled: true,
                loaded: true,
                api_name: None,
                display_name: None,
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
                model: Some("llama3".to_string()),
                quant: None,
                enabled: true,
                loaded: true,
                api_name: None,
                display_name: None,
            },
            ModelEntry {
                id: "model-1".to_string(),
                backend: "llama.cpp".to_string(),
                model: Some("llama2".to_string()),
                quant: None,
                enabled: true,
                loaded: true,
                api_name: None,
                display_name: None,
            },
            ModelEntry {
                id: "model-4".to_string(),
                backend: "llama.cpp".to_string(),
                model: Some("llama5".to_string()),
                quant: None,
                enabled: true,
                loaded: false,
                api_name: None,
                display_name: None,
            },
            ModelEntry {
                id: "model-3".to_string(),
                backend: "llama.cpp".to_string(),
                model: Some("llama4".to_string()),
                quant: None,
                enabled: true,
                loaded: false,
                api_name: None,
                display_name: None,
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
