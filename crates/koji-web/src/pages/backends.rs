//! Backends page – manage inference backend installations.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::backend_card::{BackendCard, BackendCardDto};
use crate::components::install_modal::{CapabilitiesDto, InstallModal, InstallRequest};
use crate::components::job_log_panel::JobLogPanel;

#[derive(Debug, Clone, Deserialize, Default)]
struct BackendListResponse {
    #[serde(default)]
    backends: Vec<BackendCardDto>,
    #[serde(default)]
    custom: Vec<BackendCardDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct InstallResponse {
    job_id: String,
}

/// Top-level Backends page, reachable via the nav bar.
#[component]
pub fn Backends() -> impl IntoView {
    // ── State ────────────────────────────────────────────────────────────────
    let backends_list = RwSignal::new(BackendListResponse::default());
    let capabilities = RwSignal::new(CapabilitiesDto::default());
    let install_modal_for = RwSignal::new(Option::<String>::None);
    let active_job_id = RwSignal::new(Option::<String>::None);
    let action_error = RwSignal::new(Option::<String>::None);
    let refresh_tick = RwSignal::new(0u32);
    let default_args_edits: RwSignal<std::collections::HashMap<String, String>> =
        RwSignal::new(std::collections::HashMap::new());
    let save_status: RwSignal<Option<String>> = RwSignal::new(None);
    let saving: RwSignal<bool> = RwSignal::new(false);

    // ── Fetch backends list (re-runs on refresh_tick) ────────────────────────
    Effect::new(move |_| {
        let _ = refresh_tick.get();
        wasm_bindgen_futures::spawn_local(async move {
            match gloo_net::http::Request::get("/api/backends").send().await {
                Ok(resp) => {
                    if let Ok(list) = resp.json::<BackendListResponse>().await {
                        backends_list.set(list);
                    }
                }
                Err(e) => leptos::logging::warn!("Failed to fetch backends: {e:?}"),
            }
        });
    });

    // ── Fetch capabilities once ──────────────────────────────────────────────
    Effect::new(move |prev: Option<()>| {
        if prev.is_some() {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match gloo_net::http::Request::get("/api/system/capabilities")
                .send()
                .await
            {
                Ok(resp) => {
                    if let Ok(caps) = resp.json::<CapabilitiesDto>().await {
                        capabilities.set(caps);
                    }
                }
                Err(e) => leptos::logging::warn!("Failed to fetch capabilities: {e:?}"),
            }
        });
    });

    // ── Callbacks ────────────────────────────────────────────────────────────
    let on_install_click = Callback::new(move |backend_type: String| {
        action_error.set(None);
        install_modal_for.set(Some(backend_type));
    });

    let on_update_click = Callback::new(move |backend_type: String| {
        action_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/api/backends/{backend_type}/update");
            match gloo_net::http::Request::post(&url).send().await {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(r) = resp.json::<InstallResponse>().await {
                            active_job_id.set(Some(r.job_id));
                        }
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        action_error.set(Some(format!("Update failed: {text}")));
                    }
                }
                Err(e) => action_error.set(Some(format!("Update request failed: {e}"))),
            }
        });
    });

    let on_check_updates_click = Callback::new(move |_backend_type: String| {
        action_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            match gloo_net::http::Request::post("/api/backends/check-updates")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(list) = resp.json::<BackendListResponse>().await {
                            backends_list.set(list);
                        }
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        action_error.set(Some(format!("Check updates failed: {text}")));
                    }
                }
                Err(e) => action_error.set(Some(format!("Check updates request failed: {e}"))),
            }
        });
    });

    let on_delete_click = Callback::new(move |backend_type: String| {
        action_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/api/backends/{backend_type}");
            match gloo_net::http::Request::delete(&url).send().await {
                Ok(resp) => {
                    if resp.ok() {
                        refresh_tick.update(|n| *n += 1);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        action_error.set(Some(format!("Uninstall failed: {text}")));
                    }
                }
                Err(e) => action_error.set(Some(format!("Uninstall request failed: {e}"))),
            }
        });
    });

    let on_install_submit = Callback::new(move |req: InstallRequest| {
        install_modal_for.set(None);
        action_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            let request = match gloo_net::http::Request::post("/api/backends/install").json(&req) {
                Ok(r) => r,
                Err(e) => {
                    action_error.set(Some(format!("Failed to encode install request: {e}")));
                    return;
                }
            };
            match request.send().await {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(r) = resp.json::<InstallResponse>().await {
                            active_job_id.set(Some(r.job_id));
                        }
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        action_error.set(Some(format!("Install failed: {text}")));
                    }
                }
                Err(e) => action_error.set(Some(format!("Install request failed: {e}"))),
            }
        });
    });

    let on_install_cancel = Callback::new(move |_: ()| {
        install_modal_for.set(None);
    });

    let on_job_close = Callback::new(move |_: ()| {
        active_job_id.set(None);
        refresh_tick.update(|n| *n += 1);
    });

    let on_default_args_change =
        Callback::new(move |(backend_type, new_value): (String, String)| {
            default_args_edits.update(|edits| {
                edits.insert(backend_type, new_value);
            });
            save_status.set(None); // Clear status when user makes new edits
        });

    let save = move |_| {
        if saving.get() {
            return;
        }
        let edits = default_args_edits.get();
        if edits.is_empty() {
            return;
        }
        saving.set(true);
        save_status.set(Some("Saving…".to_string()));
        wasm_bindgen_futures::spawn_local(async move {
            let mut errors = Vec::new();
            let edit_keys: Vec<String> = edits.keys().cloned().collect();
            for bt in edit_keys {
                let args_str = edits.get(&bt).cloned().unwrap_or_default();
                let parts: Vec<String> = args_str.split_whitespace().map(String::from).collect();
                let body = serde_json::json!({ "default_args": parts });
                let url = format!("/api/backends/{}/default-args", bt);
                let res = gloo_net::http::Request::post(&url)
                    .json(&body)
                    .unwrap()
                    .send()
                    .await;
                match res {
                    Ok(response) if response.ok() => {}
                    Ok(response) => {
                        let status = response.status();
                        let text = response.text().await.unwrap_or_default();
                        errors.push(format!("{}: HTTP {} - {}", bt, status, text));
                    }
                    Err(e) => errors.push(format!("{}: {}", bt, e)),
                }
            }
            if errors.is_empty() {
                save_status.set(Some("✅ Saved".to_string()));
                default_args_edits.set(std::collections::HashMap::new());
                refresh_tick.update(|n| *n += 1);
            } else {
                save_status.set(Some(format!("❌ {}", errors.join(", "))));
            }
            saving.set(false);
        });
    };

    // ── View ─────────────────────────────────────────────────────────────────
    view! {
        <div class="page-header">
            <h1>"Backends"</h1>
            {move || {
                if !default_args_edits.get().is_empty() || saving.get() {
                    view! {
                        <div style="display:flex;gap:0.5rem;align-items:center;">
                            {move || save_status.get().map(|s| view! { <span class="text-muted">{s}</span> })}
                            <button
                                class="btn btn-primary"
                                disabled=move || saving.get()
                                on:click=save
                            >
                                "Save Changes"
                            </button>
                        </div>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            }}
        </div>

        <div class="card">
            <p class="text-muted">"Manage inference backend installations."</p>

            {/* Error banner */}
            {move || {
                if let Some(err) = action_error.get() {
                    view! {
                        <div style="background:#fee2e2;border:1px solid #ef4444;color:#b91c1c;padding:0.75rem;border-radius:4px;margin-bottom:1rem;font-size:0.875rem;">
                            {err}
                        </div>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            }}

            {/* Active job log panel */}
            {move || {
                if let Some(jid) = active_job_id.get() {
                    view! {
                        <JobLogPanel job_id=jid on_close=on_job_close />
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            }}

            {/* Backend cards */}
            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                {move || {
                    let list = backends_list.get();
                    let mut cards = Vec::new();
                    for backend in list.backends.into_iter().chain(list.custom.into_iter()) {
                        cards.push(view! {
                            <BackendCard
                                backend=backend
                                on_install=on_install_click
                                on_update=on_update_click
                                on_check_updates=on_check_updates_click
                                on_delete=on_delete_click
                                on_default_args_change=on_default_args_change
                            />
                        }.into_any());
                    }
                    cards
                }}
            </div>

            {/* Install modal */}
            {move || {
                if let Some(bt) = install_modal_for.get() {
                    let caps = capabilities.get();
                    view! {
                        <InstallModal
                            backend_type=bt
                            capabilities=caps
                            on_submit=on_install_submit
                            on_cancel=on_install_cancel
                        />
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }
            }}
        </div>
    }
}
