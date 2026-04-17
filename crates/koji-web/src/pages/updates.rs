use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::self_update_section::SelfUpdateSection;

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
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/api/backends/{}/update", name);
            let _ = gloo_net::http::Request::post(&url).send().await;
        });
    };

    let on_refresh_model = move |id: String| {
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/api/models/{}/refresh", id);
            let _ = gloo_net::http::Request::post(&url).send().await;
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

            <section class="updates-section">
                <h2 class="section__title">"Models"</h2>
                <div class="updates-list">
                    {move || {
                        let models = updates.with(|u| u.models.clone());
                        models.into_iter().map(|m| {
                            let display_name = m.display_name
                                .clone()
                                .or_else(|| m.repo_id.clone())
                                .unwrap_or_else(|| m.item_id.clone());
                            view! {
                                <div class="update-item" class:update-available=m.update_available>
                                    <div class="update-item__info">
                                        <span class="update-item__name">{display_name}</span>
                                        <span class="update-item__version">
                                            {m.current_version.as_ref().map(|v| &v[..8.min(v.len())]).unwrap_or("—").to_string()}
                                        </span >
                                        {if m.update_available {
                                            let latest = m.latest_version.as_ref().map(|v| &v[..8.min(v.len())]).unwrap_or("").to_string();
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
                                        {if m.update_available {
                                            let id = m.item_id.clone();
                                            view! {
                                                <button class="btn btn-secondary"
                                                    on:click=move |_| on_refresh_model(id.clone())>
                                                    "Re-pull"
                                                </button>
                                            }.into_any()
                                        } else {
                                            view! { <span/> }.into_any()
                                        }}
                                        <a href=format!("/models/{}", m.item_id) class="btn btn-ghost">
                                            "Edit"
                                        </a>
                                    </div >
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}
                </div>
            </section>
        </div>
    }
}
