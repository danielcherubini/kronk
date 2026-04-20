use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::job_log_panel::JobLogPanel;
use crate::components::self_update_section::SelfUpdateSection;

fn short_sha(hash: &Option<String>) -> String {
    match hash {
        Some(h) => h.chars().take(8).collect(),
        None => "—".to_string(),
    }
}

/// Renders the expandable quant list for a model.
/// Called from within the Updates component view; captures signals via params.
/// Parsed quant detail from details_json: (quant_name, filename, current_hash, latest_hash, update_available)
type QuantRow = (Option<String>, String, Option<String>, Option<String>, bool);

fn render_quant_list(
    mid: String,
    quants: Vec<(String, Option<String>, Option<String>, bool)>,
    selections: RwSignal<std::collections::HashMap<String, std::collections::HashSet<String>>>,
    update_busy: RwSignal<Option<String>>,
    on_select_all: impl Fn() + 'static,
    on_update_selected: impl Fn(String) + 'static,
) -> impl IntoView {
    view! {
        <div class="quant-list" style="margin-top:0.5rem;padding-left:1.5rem;">
            {/* Select All button */}
            <div style="display:flex;gap:0.5rem;margin-bottom:0.5rem;">
                <button
                    class="btn btn-ghost btn-sm"
                    style="font-size:0.75rem;padding:0.125rem 0.5rem;"
                    on:click=move |_| on_select_all()
                >
                    "Select All"
                </button>
            </div>

            {/* Quant rows */}
            {quants.into_iter().map(|(quant_name, current_hash, latest_hash, update_available)| {
                let qn = quant_name.clone();
                let mid_for_sel = mid.clone();
                let qn_clone = qn.clone();
                let mid_clone = mid.clone();
                let qn_change = qn.clone();
                let mid_change = mid_for_sel.clone();
                let is_selected = move || {
                    selections.with(|map| map.get(&mid_clone)
                        .map(|set| set.contains(&qn_clone)).unwrap_or(false))
                };
                view! {
                    <label class="quant-item" style="display:flex;align-items:center;gap:0.5rem;padding:0.25rem 0;">
                        <input
                            type="checkbox"
                            prop:checked=is_selected
                            disabled={!update_available}
                            on:change=move |e| {
                                use wasm_bindgen::JsCast;
                                let checked = e.target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                    .map(|el| el.checked())
                                    .unwrap_or(false);
                                if checked {
                                    selections.update(|map| {
                                        map.entry(mid_change.clone())
                                            .or_insert_with(std::collections::HashSet::new)
                                            .insert(qn_change.clone());
                                    });
                                } else {
                                    selections.update(|map| {
                                        if let Some(set) = map.get_mut(&mid_change) {
                                            set.remove(&qn_change);
                                        }
                                    });
                                }
                            }
                        />
                        <span style="font-weight:500;">{quant_name}</span>
                        <span class="text-muted" style="font-size:0.75rem;">{short_sha(&current_hash)}</span>
                        <span style="color:#94a3b8;">"→"</span>
                        <span class="text-muted" style="font-size:0.75rem;">{short_sha(&latest_hash)}</span>
                        {if update_available {
                            view! { <span class="badge" style="background:#f59e0b;color:white;padding:0.125rem 0.375rem;border-radius:4px;font-size:0.625rem;">"Update"</span> }.into_any()
                        } else {
                            view! { <span class="badge" style="background:#22c55e;color:white;padding:0.125rem 0.375rem;border-radius:4px;font-size:0.625rem;">"Up to date"</span> }.into_any()
                        }}
                    </label>
                }.into_any()
            }).collect::<Vec<_>>()}

            {/* Update Selected button */}
            <button
                class="btn btn-primary btn-sm"
                style="margin-top:0.5rem;"
                disabled={
                    let mid_ref = mid.clone();
                    move || {
                        update_busy.with(|b| b.as_ref().map(|id| id == &mid_ref).unwrap_or(false))
                            || selections.with(|map| map.get(&mid_ref)
                                .map(|set| set.is_empty()).unwrap_or(true))
                    }
                }
                on:click=move |_| on_update_selected(mid.clone())
            >
                {let mid_ref = mid.clone(); move || if update_busy.with(|b| b.as_ref().map(|id| id == &mid_ref).unwrap_or(false)) { "Updating...".to_string() } else { "Update Selected".to_string() }}
            </button>
        </div>
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpdateCheckDto {
    pub item_type: String,
    pub item_id: String,
    pub repo_id: Option<String>,
    pub display_name: Option<String>,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub status: String,
    pub error_message: Option<String>,
    pub checked_at: i64,
    pub details_json: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpdatesListResponse {
    pub backends: Vec<UpdateCheckDto>,
    pub models: Vec<UpdateCheckDto>,
}

#[component]
pub fn Updates() -> impl IntoView {
    let updates = RwSignal::new(UpdatesListResponse {
        backends: vec![],
        models: vec![],
    });
    let checking = RwSignal::new(false);
    let last_checked = RwSignal::new(Option::<i64>::None);
    let error = RwSignal::new(Option::<String>::None);
    let active_backend_job_id = RwSignal::new(Option::<String>::None);
    let backend_update_busy = RwSignal::new(false);

    // Tracks which models have their quant list expanded (model_id → bool)
    let model_expanded: RwSignal<std::collections::HashMap<String, bool>> =
        RwSignal::new(std::collections::HashMap::new());

    // Tracks selected quants per model (model_id → HashSet of quant keys)
    let model_selections: RwSignal<
        std::collections::HashMap<String, std::collections::HashSet<String>>,
    > = RwSignal::new(std::collections::HashMap::new());

    // Busy state for model update action (model_id → bool)
    let model_update_busy = RwSignal::new(Option::<String>::None);

    // Fetch on mount
    Effect::new(move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            match gloo_net::http::Request::get("/api/updates").send().await {
                Ok(resp) if resp.ok() => {
                    if let Ok(data) = resp.json::<UpdatesListResponse>().await {
                        updates.set(data.clone());
                        // Get last checked time from any record
                        let all_items: Vec<_> =
                            data.backends.iter().chain(data.models.iter()).collect();
                        last_checked.set(all_items.iter().map(|r| r.checked_at).max());
                    }
                }
                _ => error.set(Some("Failed to load updates".to_string())),
            }
        });
    });

    let on_check_now = move |_| {
        checking.set(true);
        error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            match gloo_net::http::Request::post("/api/updates/check")
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    // Refresh list after a delay
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    if let Ok(resp2) = gloo_net::http::Request::get("/api/updates").send().await {
                        if let Ok(data) = resp2.json::<UpdatesListResponse>().await {
                            updates.set(data);
                        }
                    }
                }
                _ => error.set(Some("Failed to trigger check".to_string())),
            }
            checking.set(false);
        });
    };

    let on_update_backend = move |name: String| {
        backend_update_busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/api/backends/{}/update", name);
            if let Ok(resp) = gloo_net::http::Request::post(&url).send().await {
                if resp.ok() {
                    if let Ok(data) = resp.json::<serde_json::Value>().await {
                        if let Some(job_id) = data["job_id"].as_str() {
                            active_backend_job_id.set(Some(job_id.to_string()));
                        }
                    }
                } else {
                    let text = resp.text().await.unwrap_or_default();
                    error.set(Some(format!("Update failed: {}", text)));
                }
            }
        });
    };

    let on_backend_job_close = Callback::new(move |_| {
        active_backend_job_id.set(None);
        backend_update_busy.set(false);
        // Refresh the updates list after job completes
        wasm_bindgen_futures::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(500).await;
            if let Ok(resp) = gloo_net::http::Request::get("/api/updates").send().await {
                if let Ok(data) = resp.json::<UpdatesListResponse>().await {
                    let all_items: Vec<_> =
                        data.backends.iter().chain(data.models.iter()).collect();
                    last_checked.set(all_items.iter().map(|r| r.checked_at).max());
                    updates.set(data);
                }
            }
        });
    });

    let _on_refresh_model = move |id: String| {
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/api/models/{}/refresh", id);
            let _ = gloo_net::http::Request::post(&url).send().await;
        });
    };

    let on_toggle_expand = move |model_id: String| {
        model_expanded.update(|map| {
            map.entry(model_id).and_modify(|v| *v = !*v).or_insert(true);
        });
    };

    let on_update_selected = move |model_id: String| {
        // Read selections inside the async block (not before spawn — avoids unused capture)
        model_update_busy.set(Some(model_id.clone()));
        wasm_bindgen_futures::spawn_local(async move {
            let selected_quants: Vec<String> = model_selections
                .get()
                .get(&model_id)
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default();

            if selected_quants.is_empty() {
                model_update_busy.set(None);
                return;
            }

            let url = format!("/api/updates/apply/model/{}", model_id);
            match gloo_net::http::Request::post(&url)
                .json(&serde_json::json!({ "quants": selected_quants }))
                .unwrap()
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    // Clear selections for this model immediately
                    model_selections.update(|map| {
                        map.remove(&model_id);
                    });
                    // Refresh list after delay
                    wasm_bindgen_futures::spawn_local(async move {
                        gloo_timers::future::TimeoutFuture::new(2000).await;
                        if let Ok(r) = gloo_net::http::Request::get("/api/updates").send().await {
                            if let Ok(data) = r.json::<UpdatesListResponse>().await {
                                updates.set(data);
                            }
                        }
                    });
                }
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status == 409 {
                        error.set(Some(format!("Download already in progress: {}", text)));
                    } else if status == 422 {
                        error.set(Some(format!("Invalid quant keys: {}", text)));
                    } else {
                        error.set(Some(format!("Update failed: {}", text)));
                    }
                }
                Err(e) => error.set(Some(format!("Request failed: {}", e))),
            }
            model_update_busy.set(None);
        });
    };

    view! {
        <div class="page updates-page">
            <h1 class="page__title">"Updates Center"</h1>

            <div class="updates-header">
                <button
                    class="btn btn-primary"
                    disabled=move || checking.get()
                    on:click=on_check_now
                >
                    {move || if checking.get() { "Checking..." } else { "Check Now" }}
                </button>
                {move || last_checked.get().map(|ts| {
                    let date = chrono::DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    view! { <span class="last-checked">"Last checked: " {date}</span> }
                })}
            </div>

            {move || error.get().map(|e| view! {
                <div class="error-banner">{e}</div>
            })}

            // Self-update section for the Koji application itself
            <SelfUpdateSection />

            <section class="updates-section">
                <h2 class="section__title">"Backends"</h2>
                <div class="updates-list">
                    {move || {
                        let backends = updates.with(|u| u.backends.clone());
                        backends.into_iter().map(|b| {
                            view! {
                                <div class="update-item" class:update-available=b.update_available>
                                    <div class="update-item__info">
                                        <span class="update-item__name">{b.item_id.clone()}</span>
                                        <span class="update-item__version">
                                            {b.current_version.clone().unwrap_or_else(|| "—".to_string())}
                                        </span >
                                        {if b.update_available {
                                            let latest = b.latest_version.clone().unwrap_or_default();
                                            view! {
                                                <span class="update-badge">
                                                    {format!(" → {}", latest)}
                                                </span >
                                            }.into_any()
                                        } else {
                                            view! { <span class="up-to-date-badge">{"✓ Up to date"}</span> }.into_any()
                                        }}
                                    </div >
                                    <div class="update-item__actions">
                                        {if b.update_available {
                                            let id = b.item_id.clone();
                                            view! {
                                                <button class="btn btn-secondary"
                                                    on:click=move |_| on_update_backend(id.clone())>
                                                    "Update"
                                                </button>
                                            }.into_any()
                                        } else {
                                            view! { <span/> }.into_any()
                                        }}
                                        <button class="btn btn-ghost"
                                            on:click=move |_| {
                                                let id = b.item_id.clone();
                                                wasm_bindgen_futures::spawn_local(async move {
                                                    let url = format!("/api/updates/check/backend/{}", id);
                                                    let _ = gloo_net::http::Request::post(&url).send().await;
                                                });
                                            }>
                                            "Refresh"
                                        </button>
                                    </div >
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}
                </div>
            </section>

            {/* Backend update progress panel */}
            {move || active_backend_job_id.get().map(|job_id| {
                view! {
                    <JobLogPanel job_id=job_id on_close=on_backend_job_close />
                }.into_any()
            })}

            <section class="updates-section">
                <h2 class="section__title">"Models"</h2>
                <div class="updates-list">
                    {move || {
                        let models = updates.with(|u| u.models.clone());
                        models.into_iter().map(|m| {
                            let model_id = m.item_id.clone();
                            let display_name = m.display_name
                                .clone()
                                .or_else(|| m.repo_id.clone())
                                .unwrap_or_else(|| m.item_id.clone());

                            // Parse quants from details_json (same pattern as get_updates in api/updates.rs)
                            // Use Option<String> for quant_name to preserve entries where it's null (e.g., new remote files)
                            let quants_with_updates: Vec<QuantRow> = m.details_json
                                .as_ref()
                                .and_then(|d| d.get("quants"))
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|q| {
                                            let quant_name = q["quant_name"].as_str().map(String::from);
                                            let filename = q["filename"].as_str()?.to_string();
                                            let current_hash = q["current_hash"].as_str().map(String::from);
                                            let latest_hash = q["latest_hash"].as_str().map(String::from);
                                            let update_available = q["update_available"].as_bool()?;
                                            Some((quant_name, filename, current_hash, latest_hash, update_available))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            // Clone model_id for use in nested closures (avoids FnOnce issue)
                            let mid_expand = model_id.clone();

                            // Owned copy for the select-all callback (use quant_name or fallback to filename)
                            let quants_for_select_owned: Vec<(String, bool)> = quants_with_updates
                                .iter()
                                .map(|(qn, filename, _, _, u)| {
                                    (
                                        qn.clone().unwrap_or_else(|| filename.clone()),
                                        *u,
                                    )
                                })
                                .collect();

                            let has_updates = quants_with_updates.iter().any(|(_, _, _, _, u)| *u);

                            view! {
                                <div class="update-item" class:update-available=has_updates>
                                    {/* Model header with expand/collapse chevron */}
                                    <div class="update-item__info">
                                        <span
                                            class="expand-toggle"
                                            style="cursor:pointer;margin-right:0.5rem;font-size:0.75rem;"
                                            on:click=move |_| on_toggle_expand(model_id.clone())
                                        >
                                            {let mid_chev = mid_expand.clone(); move || {
                                                match model_expanded.get().get(&mid_chev) {
                                                    Some(&v) => if v { "▼".to_string() } else { "▶".to_string() },
                                                    None => "▶".to_string(),
                                                }
                                            }}
                                        </span>
                                        <span class="update-item__name">{display_name}</span>
                                        {/* version info */}
                                        {m.current_version.as_ref().map(|v| {
                                            let ver = v[..8.min(v.len())].to_string();
                                            view! {
                                                <span class="update-item__version">
                                                    {ver}
                                                </span>
                                            }
                                        })}
                                        {if has_updates {
                                            let latest = m.latest_version.as_ref().map(|v| &v[..8.min(v.len())]).unwrap_or("").to_string();
                                            view! {
                                                <span class="update-badge">
                                                    {format!(" → {}", latest)}
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! { <span class="up-to-date-badge">{"✓ Up to date"}</span> }.into_any()
                                        }}
                                    </div>

                                    {/* Expandable quant list */}
                                    {let mid_for_cond = mid_expand.clone();
                                     let expanded = model_expanded.with(|map| map.get(&mid_for_cond).copied().unwrap_or(false));
                                     if expanded {
                                        // Prepare owned data for the helper function
                                        let mid_sel = mid_expand.clone();
                                        let mid_select_all = mid_expand.clone();
                                        let quants_owned: Vec<(String, Option<String>, Option<String>, bool)> =
                                            quants_with_updates.iter().map(|(qn, filename, ch, lh, u)| {
                                                (
                                                    qn.clone().unwrap_or_else(|| filename.clone()),
                                                    ch.clone(),
                                                    lh.clone(),
                                                    *u,
                                                )
                                            }).collect();
                                        let on_select_all_cb = move || {
                                            model_selections.update(|map| {
                                                let set: std::collections::HashSet<String> = quants_for_select_owned
                                                    .iter()
                                                    .filter(|(_, u)| *u)
                                                    .map(|(k, _)| k.clone())
                                                    .collect();
                                                map.insert(mid_select_all.clone(), set);
                                            });
                                        };
                                        render_quant_list(
                                            mid_sel,
                                            quants_owned,
                                            model_selections,
                                            model_update_busy,
                                            on_select_all_cb,
                                            on_update_selected,
                                        ).into_any()
                                    } else {
                                        view! { <span/> }.into_any()
                                    }}

                                    {/* Legacy action buttons — keep for backward compat */}
                                    <div class="update-item__actions">
                                        {if has_updates {
                                            let id = m.item_id.clone();
                                            view! {
                                                <button class="btn btn-secondary"
                                                    on:click=move |_| wasm_bindgen_futures::spawn_local({
                                                        let url_id = id.clone();
                                                        async move {
                                                            let url = format!("/api/models/{}/refresh", url_id);
                                                            let _ = gloo_net::http::Request::post(&url).send().await;
                                                        }
                                                    })>
                                                    "Refresh Metadata"
                                                </button>
                                            }.into_any()
                                        } else {
                                            view! { <span/> }.into_any()
                                        }}
                                        <a href=format!("/models/{}", m.item_id) class="btn btn-ghost">
                                            "Edit"
                                        </a>
                                    </div>
                                </div>
                            }.into_any()
                        }).collect::<Vec<_>>()
                    }}
                </div>
            </section>
        </div>
    }
}
