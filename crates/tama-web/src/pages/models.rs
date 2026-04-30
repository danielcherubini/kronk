use leptos::prelude::*;
use leptos_router::components::A;
use serde::{Deserialize, Serialize};

use crate::components::modal::Modal;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
use crate::utils::{post_request, rw_signal_to_signal, CheckAllModelsApiResponse};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelEntry {
    id: i64,
    backend: String,
    model: Option<String>,
    quant: Option<String>,
    enabled: bool,
    #[serde(default)]
    loaded: bool,
    /// Lifecycle state: idle, loading, ready, unloading, failed.
    #[serde(default)]
    state: String,
    #[serde(default)]
    api_name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsResponse {
    models: Vec<ModelEntry>,
}

/// Returns the preferred display name for a model, preferring `display_name`,
/// then `api_name`, falling back to the model `id` otherwise.
fn model_display_name(m: &ModelEntry) -> String {
    m.display_name
        .clone()
        .or(m.api_name.clone())
        .unwrap_or_else(|| m.id.to_string())
}

/// Map a model's state string to (display label, CSS badge class).
/// Falls back to `loaded` flag when `state` is empty (API doesn't always return it).
fn model_state_badge(state: &str, loaded: bool) -> (&'static str, &'static str) {
    match state {
        "ready" => ("Loaded", "badge-success"),
        "loading" => ("Loading", "badge-info"),
        "unloading" => ("Unloading", "badge-warning"),
        "failed" => ("Failed", "badge-error"),
        _ if loaded => ("Loaded", "badge-success"),
        _ => ("Idle", "badge-muted"),
    }
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
        let resp = gloo_net::http::Request::get("/tama/v1/models")
            .send()
            .await
            .ok()?;
        resp.json::<ModelsResponse>().await.ok()
    });

    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            let _ = post_request(&format!("/tama/v1/models/{}/load", id))
                .send()
                .await;
            refresh.update(|n| *n += 1);
        }
    });

    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            let _ = post_request(&format!("/tama/v1/models/{}/unload", id))
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
            let resp = match gloo_net::http::Request::get("/tama/v1/models").send().await {
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
            let list = match resp.json::<CheckAllModelsApiResponse>().await {
                Ok(v) => v,
                Err(e) => {
                    check_all_status
                        .set(Some((false, format!("Failed to parse models list: {}", e))));
                    check_all_busy.set(false);
                    return;
                }
            };

            let ids: Vec<i64> = list.models.iter().map(|m| m.id).collect();

            let total = ids.len();
            let mut ok_count = 0usize;
            let mut failed = Vec::<String>::new();
            for id in ids {
                let url = format!("/tama/v1/models/{}/refresh", id);
                match post_request(&url).send().await {
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
                            view! {
                                <div class="models-list">
                                    {data.models.into_iter().map(|m| {
                                        let id_load = m.id.to_string();
                                        let id_unload = m.id.to_string();
                                        let id_edit = m.id.to_string();
                                        let enabled_class = if m.enabled { "badge badge-success" } else { "badge badge-warning" };
                                        let (state_label, state_class) = model_state_badge(&m.state, m.loaded);
                                        view! {
                                            <div class="model-row card">
                                                <span class="model-row__name">{model_display_name(&m)}</span>
                                                <span class="model-row__meta">{m.quant.clone().unwrap_or_else(|| "\u{2014}".to_string())}</span>
                                                <span class="model-row__backend text-mono">{m.backend}</span>
                                                <div class="model-row__actions">
                                                    <span class=enabled_class>
                                                        {if m.enabled { "Enabled" } else { "Disabled" }}
                                                    </span>
                                                    <span class={format!("badge {}", state_class)}>{state_label}</span>
                                                    {if m.loaded {
                                                        view! {
                                                            <button
                                                                class="btn btn-danger btn-sm"
                                                                on:click=move |_| {
                                                                    unload_action.dispatch(id_unload.clone());
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
                                                                }
                                                            >
                                                                "Load"
                                                            </button>
                                                        }.into_any()
                                                    }}
                                                    <A
                                                        href=format!("/models/{}/edit", id_edit)
                                                        attr:class="btn btn-secondary btn-sm"
                                                    >
                                                        "Edit"
                                                    </A>
                                                </div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
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
