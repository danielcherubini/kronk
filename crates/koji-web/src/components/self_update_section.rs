use gloo_net::http::Request;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::utils::self_update::stream_update_events;

/// Self-update section component for the /updates page.
///
/// Displays the current version, check for updates, and provides an
/// update flow with confirmation dialog, inline progress, and SSE streaming.
#[component]
pub fn SelfUpdateSection() -> impl IntoView {
    let current_version = RwSignal::new(String::new());
    let update_available = RwSignal::new(false);
    let latest_version = RwSignal::new(String::new());
    let update_in_progress = RwSignal::new(false);
    let update_status = RwSignal::new(String::new());
    let show_update_confirm = RwSignal::new(false);
    let check_error = RwSignal::new(Option::<String>::None);

    // Initial check for updates on mount
    let check_for_updates = move || {
        spawn_local(async move {
            match Request::get("/api/self-update/check").send().await {
                Ok(resp) if resp.ok() => {
                    if let Ok(data) = resp.json::<serde_json::Value>().await {
                        if let Some(v) = data["current_version"].as_str() {
                            current_version.set(v.to_string());
                        }
                        let has_update = data["update_available"].as_bool() == Some(true);
                        update_available.set(has_update);
                        if has_update {
                            if let Some(v) = data["latest_version"].as_str() {
                                latest_version.set(v.to_string());
                            }
                        }
                        check_error.set(None);
                    } else {
                        check_error.set(Some("Unable to parse update response".to_string()));
                    }
                }
                Ok(resp) => {
                    let msg = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());
                    check_error.set(Some(format!("Unable to check for updates: {}", msg)));
                }
                Err(e) => {
                    check_error.set(Some(format!("Unable to check for updates: {}", e)));
                }
            }
        });
    };

    // Run initial check on mount
    Effect::new(move |_| {
        check_for_updates();
    });

    let retry_check = move || {
        check_error.set(None);
        check_for_updates();
    };

    let confirm_update = move |_| {
        show_update_confirm.set(false);
        update_in_progress.set(true);
        update_status.set("Starting update...".to_string());

        spawn_local(async move {
            // Step 1: POST to trigger the update
            match Request::post("/api/self-update/update").send().await {
                Ok(resp) if resp.ok() => {
                    // Step 2: Open SSE to stream progress
                    stream_update_events(
                        update_status,
                        update_in_progress,
                        update_available,
                        current_version,
                        latest_version,
                    )
                    .await;
                }
                Ok(resp) => {
                    // Check for 409 conflict (already in progress)
                    if resp.status() == 409 {
                        check_error.set(Some("An update is already in progress.".to_string()));
                        update_in_progress.set(false);
                    } else {
                        let msg = resp
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        check_error.set(Some(format!("Failed to start update: {}", msg)));
                        update_in_progress.set(false);
                    }
                }
                Err(e) => {
                    check_error.set(Some(format!("Failed to start update: {}", e)));
                    update_in_progress.set(false);
                }
            }
        });
    };

    view! {
        <div class="self-update-section">
            <h2 class="section__title">"Koji"</h2>

            // Loading state (initial check in flight)
            {move || (current_version.with(|v| v.is_empty()) && check_error.get().is_none() && !update_in_progress.get()).then(|| view! {
                <div class="self-update-progress">
                    <div class="self-update-spinner"></div>
                    <span>"Checking for updates…"</span>
                </div>
            })}

            // Inline progress during update
            {move || update_in_progress.get().then(|| view! {
                <div class="self-update-progress">
                    <div class="self-update-spinner"></div>
                    <span>{move || update_status.get()}</span>
                </div>
            })}

            // Error state with retry (only when not in progress)
            {move || (!update_in_progress.get() && check_error.get().is_some()).then(|| view! {
                <div class="self-update-error">
                    <span>{move || check_error.get().clone().unwrap_or_default()}</span>
                    <button class="btn btn-ghost" on:click=move |_| retry_check()>"Retry"</button>
                </div>
            })}

            // Normal state (no error, not in progress, not loading)
            {move || (!update_in_progress.get() && check_error.get().is_none() && !current_version.with(|v| v.is_empty())).then(|| view! {
                <div class="self-update-info">
                    <span class="self-update-version">
                        {move || {
                            let cv = current_version.get();
                            if update_available.get() && !cv.is_empty() {
                                format!("v{} → v{}", cv, latest_version.get())
                            } else {
                                format!("v{}", cv)
                            }
                        }}
                    </span>
                    {move || (update_available.get() && !update_in_progress.get()).then(|| view! {
                        <button class="btn btn-primary" disabled=move || update_in_progress.get()
                            on:click=move |_| show_update_confirm.set(true)>
                            "Update"
                        </button>
                    })}
                </div>
            })}
        </div>

        // Confirmation dialog (page-scoped overlay, shown BEFORE update starts)
        {move || show_update_confirm.get().then(|| view! {
            <div class="update-confirm-overlay">
                <div class="update-confirm-dialog">
                    <p>{format!("Update Koji to v{}?", latest_version.get())}</p>
                    <p class="update-confirm-note">"Koji will restart after updating."</p>
                    <div class="update-confirm-actions">
                        <button class="btn btn-secondary" on:click=move |_| show_update_confirm.set(false)>"Cancel"</button>
                        <button class="btn btn-primary" on:click=confirm_update>"Update"</button>
                    </div>
                </div>
            </div>
        })}
    }
}
