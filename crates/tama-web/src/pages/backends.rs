//! Backends page – manage inference backend installations.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::backend_card::{BackendCard, BackendCardDto};
use crate::components::install_modal::{CapabilitiesDto, InstallModal, InstallRequest};
use crate::components::job_log_panel::JobLogPanel;
use crate::utils::{extract_and_store_csrf_token, post_request};

#[derive(Debug, Clone, Deserialize, Default)]
struct BackendListResponse {
    #[serde(default)]
    backends: Vec<BackendCardDto>,
    #[serde(default)]
    custom: Vec<BackendCardDto>,
    #[serde(default)]
    available: Vec<String>,
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
    let show_backend_dropdown = RwSignal::new(false);

    // ── Fetch backends list (re-runs on refresh_tick) ────────────────────────
    Effect::new(move |_| {
        let _ = refresh_tick.get();
        wasm_bindgen_futures::spawn_local(async move {
            match gloo_net::http::Request::get("/tama/v1/backends")
                .send()
                .await
            {
                Ok(resp) => {
                    // Store CSRF token from response header (fallback when cookie unavailable)
                    extract_and_store_csrf_token(&resp);
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
            match gloo_net::http::Request::get("/tama/v1/system/capabilities")
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

    let on_update_click = Callback::new(move |(backend_type, gpu_variant): (String, String)| {
        action_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/tama/v1/backends/{backend_type}/update?gpu_variant={gpu_variant}");
            match post_request(&url).send().await {
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

    let on_check_updates_click =
        Callback::new(move |(backend_type, gpu_variant): (String, String)| {
            action_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                // Check a single backend variant via the updates API
                let url = format!(
                    "/tama/v1/updates/check/backend/{}?gpu_variant={}",
                    backend_type, gpu_variant
                );
                match post_request(&url).send().await {
                    Ok(resp) => {
                        if resp.ok() {
                            // After checking, refresh the full backend list to get updated status
                            match gloo_net::http::Request::get("/tama/v1/backends")
                                .send()
                                .await
                            {
                                Ok(resp2) => {
                                    if let Ok(list) = resp2.json::<BackendListResponse>().await {
                                        backends_list.set(list);
                                    }
                                }
                                Err(e) => action_error
                                    .set(Some(format!("Failed to refresh backends: {e}"))),
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

    let on_delete_click = Callback::new(move |(backend_type, gpu_variant): (String, String)| {
        action_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/tama/v1/backends/{backend_type}?gpu_variant={gpu_variant}");
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
            let request = match post_request("/tama/v1/backends/install").json(&req) {
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

    // Key by "backend_type:gpu_variant" so each variant has its own args.
    // e.g. "llama_cpp:vulkan" vs "llama_cpp:rocm"
    let on_default_args_change =
        Callback::new(move |(backend_key, new_value): (String, String)| {
            default_args_edits.update(|edits| {
                edits.insert(backend_key, new_value);
            });
            save_status.set(None); // Clear status when user makes new edits
        });

    // Track version selection changes: key = "backend_type:gpu_variant", value = (type, version, variant)
    let version_edits: RwSignal<std::collections::HashMap<String, (String, String, String)>> =
        RwSignal::new(std::collections::HashMap::new());

    let on_version_change = Callback::new(
        move |(backend_type, version, gpu_variant): (String, String, String)| {
            let key = format!("{}:{}", backend_type, gpu_variant);
            version_edits.update(|edits| {
                edits.insert(key, (backend_type, version, gpu_variant));
            });
            save_status.set(None);
        },
    );

    let save = move |_| {
        if saving.get() {
            return;
        }
        let args_edits = default_args_edits.get();
        let ver_edits = version_edits.get();
        if args_edits.is_empty() && ver_edits.is_empty() {
            return;
        }
        saving.set(true);
        save_status.set(Some("Saving…".to_string()));
        wasm_bindgen_futures::spawn_local(async move {
            let mut errors = Vec::new();

            // Apply version changes first
            for (bt, ver, gv) in ver_edits.values() {
                let url = format!("/tama/v1/backends/{}/activate?gpu_variant={}", bt, gv);
                let body = serde_json::json!({ "version": ver });
                match post_request(&url).json(&body).unwrap().send().await {
                    Ok(resp) if resp.ok() => {}
                    Ok(resp) => {
                        let status = resp.status();
                        let text = resp.text().await.unwrap_or_default();
                        errors.push(format!("Activate {}: HTTP {} - {}", bt, status, text));
                    }
                    Err(e) => errors.push(format!("Activate {}: {}", bt, e)),
                }
            }

            // Apply default args changes — key is "backend_type:gpu_variant"
            let edit_keys: Vec<String> = args_edits.keys().cloned().collect();
            for key in edit_keys {
                let args_str = args_edits.get(&key).cloned().unwrap_or_default();
                let parts: Vec<String> = args_str.split_whitespace().map(String::from).collect();
                // Parse "backend_type:gpu_variant" from key
                let parts_key: Vec<&str> = key.splitn(2, ':').collect();
                let bt = parts_key[0];
                let gv = parts_key.get(1).copied().unwrap_or("cpu");
                let body = serde_json::json!({ "default_args": parts });
                let url = format!("/tama/v1/backends/{}/default-args?gpu_variant={}", bt, gv);
                let res = post_request(&url).json(&body).unwrap().send().await;
                match res {
                    Ok(response) if response.ok() => {}
                    Ok(response) => {
                        let status = response.status();
                        let text = response.text().await.unwrap_or_default();
                        errors.push(format!("{}: HTTP {} - {}", key, status, text));
                    }
                    Err(e) => errors.push(format!("{}: {}", key, e)),
                }
            }

            if errors.is_empty() {
                save_status.set(Some("✅ Saved".to_string()));
                default_args_edits.set(std::collections::HashMap::new());
                version_edits.set(std::collections::HashMap::new());
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
            <div style="display:flex;gap:0.5rem;align-items:center;">
                {move || save_status.get().map(|s| view! { <span class="text-muted">{s}</span> })}
                <button
                    class="btn btn-primary"
                    disabled=move || saving.get()
                    on:click=save
                >
                    "Save Changes"
                </button>
                <div style="position:relative;">
                    <button
                        class="btn btn-success"
                        on:click=move |_| {
                            show_backend_dropdown.update(|v| *v = !*v);
                        }
                    >
                        "+ Add Backend"
                    </button>
                    {move || {
                        if !show_backend_dropdown.get() {
                            return view! { <span/> }.into_any();
                        }
                        let all = vec![
                            ("llama_cpp", "llama.cpp"),
                            ("ik_llama", "ik_llama.cpp"),
                            ("tts_kokoro", "Kokoro TTS"),
                        ];
                        let mut items = all;
                        items.sort_by_key(|(_, d)| *d);

                        view! {
                            <div style="position:absolute;right:0;top:100%;margin-top:4px;background:#1e293b;border:1px solid #334155;border-radius:6px;padding:0.5rem 0;z-index:100;width:200px;box-shadow:0 4px 12px rgba(0,0,0,0.3);">
                                {items.into_iter().map(|(backend_type, display_name): (&str, &str)| {
                                    let bt = backend_type.to_string();
                                    view! {
                                        <button
                                            style="width:100%;text-align:left;padding:0.5rem 0.75rem;background:none;border:none;color:#e2e8f0;cursor:pointer;font-size:0.875rem;"
                                            on:click=move |_| {
                                                action_error.set(None);
                                                install_modal_for.set(Some(bt.clone()));
                                                show_backend_dropdown.set(false);
                                            }
                                        >
                                            {display_name}
                                        </button>
                                    }.into_any()
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }}
                </div>
            </div>
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
                    let combined: Vec<_> = list.backends.into_iter()
                        .chain(list.custom.into_iter())
                        .collect();

                    if combined.is_empty() {
                        return view! {
                            <div style="text-align:center;padding:2.5rem 2rem;color:#64748b;">
                                <div style="font-size:1.125rem;font-weight:500;margin-bottom:0.5rem;">
                                    "No backends installed"
                                </div>
                                <div style="font-size:0.875rem;margin-bottom:1.5rem;">
                                    "Click the + Add Backend button to get started."
                                </div>
                            </div>
                        }.into_any();
                    }

                    let mut cards = Vec::new();
                    for backend in combined {
                        cards.push(view! {
                            <BackendCard
                                backend=backend
                                on_install=on_install_click
                                on_update=on_update_click
                                on_check_updates=on_check_updates_click
                                on_delete=on_delete_click
                                on_default_args_change=on_default_args_change
                                on_version_change=on_version_change
                            />
                        }.into_any());
                    }
                    view! { <>{cards}</> }.into_any()
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
