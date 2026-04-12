use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use leptos::prelude::*;
use leptos_router::components::A;
use serde::Deserialize;
use web_sys::window;

#[derive(Debug, Clone, Deserialize)]
struct LogPayload {
    line: String,
}

#[derive(Debug, Clone, Deserialize)]
struct StatusPayload {
    status: String,
    #[serde(default)]
    error: Option<String>,
}

#[component]
pub fn Sidebar() -> impl IntoView {
    let collapsed = RwSignal::new(false);
    let mobile_open = RwSignal::new(false);

    // Self-update signals
    let current_version = RwSignal::new(String::new());
    let update_available = RwSignal::new(false);
    let latest_version = RwSignal::new(String::new());
    let update_in_progress = RwSignal::new(false);
    let update_status = RwSignal::new(String::new());
    let show_update_confirm = RwSignal::new(false);

    // On mount, read localStorage for persisted state.
    // Use a plain closure (not Effect::new) since this has no reactive
    // dependencies.
    let initial = window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|ls| ls.get("koji-sidebar-collapsed").ok())
        .flatten();
    if initial.as_deref() == Some("true") {
        collapsed.set(true);
    }

    // Persist state when it changes — this IS reactive (subscribes to
    // collapsed).
    Effect::new(move || {
        let val = if collapsed.get() { "true" } else { "false" };
        if let (Some(_), Some(ls)) = (
            window(),
            window().and_then(|w| w.local_storage().ok()).flatten(),
        ) {
            let _ = ls.set("koji-sidebar-collapsed", val);
        }
    });

    // Check for updates on mount
    leptos::task::spawn_local(async move {
        if let Ok(resp) = gloo_net::http::Request::get("/api/self-update/check")
            .send()
            .await
        {
            if let Ok(data) = resp.json::<serde_json::Value>().await {
                if let Some(v) = data["current_version"].as_str() {
                    current_version.set(v.to_string());
                }
                if data["update_available"].as_bool() == Some(true) {
                    update_available.set(true);
                    if let Some(v) = data["latest_version"].as_str() {
                        latest_version.set(v.to_string());
                    }
                }
            }
        }
    });

    // Confirm update handler: POST trigger then GET SSE stream
    let confirm_update = move |_| {
        show_update_confirm.set(false);
        update_in_progress.set(true);
        update_status.set("Starting update...".to_string());

        leptos::task::spawn_local(async move {
            // Step 1: POST to trigger the update
            let trigger_result = gloo_net::http::Request::post("/api/self-update/update")
                .send()
                .await;

            match trigger_result {
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
                    let msg = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());
                    update_status.set(format!("Failed to start update: {}", msg));
                    update_in_progress.set(false);
                }
                Err(e) => {
                    update_status.set(format!("Failed to start update: {}", e));
                    update_in_progress.set(false);
                }
            }
        });
    };

    view! {
        // Mobile hamburger toggle (hidden on desktop)
        <button class="sidebar-mobile-toggle" on:click=move |_| mobile_open.set(true)>
            "☰"
        </button>

        // Overlay backdrop (hidden when mobile_open is false)
        <div
            class="sidebar-overlay"
            class:sidebar-overlay--visible=move || mobile_open.get()
            on:click=move |_| mobile_open.set(false)
        />

        <aside
            class="sidebar"
            class:sidebar--collapsed=move || collapsed.get()
            class:sidebar--mobile-open=move || mobile_open.get()
        >
            // Close button for mobile (hidden on desktop)
            <button class="sidebar-close" on:click=move |_| mobile_open.set(false)>
                "✕"
            </button>

            <A href="/" attr:class="sidebar-header" on:click=move |_| mobile_open.set(false)>
                <span class="sidebar-header__logo">"⚡"</span>
                <span class="sidebar-header__text">"Koji"</span>
            </A>

            <nav class="sidebar-nav">
                <A href="/" attr:class="sidebar-item" attr:data-tooltip="Dashboard" on:click=move |_| mobile_open.set(false)>
                    <span class="sidebar-item__icon">"🏠"</span>
                    <span class="sidebar-item__text">"Dashboard"</span>
                </A>
                <A href="/models" attr:class="sidebar-item" attr:data-tooltip="Models" on:click=move |_| mobile_open.set(false)>
                    <span class="sidebar-item__icon">"📦"</span>
                    <span class="sidebar-item__text">"Models"</span>
                </A>
                <A href="/backends" attr:class="sidebar-item" attr:data-tooltip="Backends" on:click=move |_| mobile_open.set(false)>
                    <span class="sidebar-item__icon">"🔧"</span>
                    <span class="sidebar-item__text">"Backends"</span>
                </A>
                <A href="/logs" attr:class="sidebar-item" attr:data-tooltip="Logs" on:click=move |_| mobile_open.set(false)>
                    <span class="sidebar-item__icon">"📋"</span>
                    <span class="sidebar-item__text">"Logs"</span>
                </A>
            </nav>

            <div class="sidebar-footer">
                <div class="sidebar-section" style="border-top:none;margin:0;padding:0;">
                    <A href="/config" attr:class="sidebar-item" attr:data-tooltip="Config" on:click=move |_| mobile_open.set(false)>
                        <span class="sidebar-item__icon">"⚙️"</span>
                        <span class="sidebar-item__text">"Config"</span>
                    </A>
                </div>

                // Version badge
                <div class="sidebar-version">
                    <span class="sidebar-version__text">
                        {move || {
                            let cv = current_version.get();
                            if update_available.get() {
                                format!("v{} → v{}", cv, latest_version.get())
                            } else if !cv.is_empty() {
                                format!("v{}", cv)
                            } else {
                                String::new()
                            }
                        }}
                    </span>
                    {move || update_available.get().then(|| view! {
                        <button
                            class="sidebar-update-btn"
                            disabled=move || update_in_progress.get()
                            on:click=move |_| show_update_confirm.set(true)
                        >
                            {move || if update_in_progress.get() { "Updating..." } else { "Update" }}
                        </button>
                    })}
                </div>

                <button class="sidebar-toggle" on:click=move |_| collapsed.update(|c| *c = !*c)>
                    <span class="sidebar-toggle__icon">"↔"</span>
                    <span class="sidebar-toggle__text">"Collapse"</span>
                </button>
            </div>
        </aside>

        // Confirmation dialog (overlay)
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

        // Progress overlay (shown during update)
        {move || update_in_progress.get().then(|| view! {
            <div class="update-progress-overlay">
                <div class="update-progress-dialog">
                    <div class="update-progress-spinner"></div>
                    <p>{move || update_status.get()}</p>
                </div>
            </div>
        })}
    }
}

/// Stream update events via SSE, matching the pattern from
/// `job_log_panel.rs`.
async fn stream_update_events(
    update_status: RwSignal<String>,
    update_in_progress: RwSignal<bool>,
    update_available: RwSignal<bool>,
    current_version: RwSignal<String>,
    latest_version: RwSignal<String>,
) {
    let mut es = match EventSource::new("/api/self-update/events") {
        Ok(es) => es,
        Err(e) => {
            update_status.set(format!("Failed to open event stream: {:?}", e));
            update_in_progress.set(false);
            return;
        }
    };

    let mut log_stream = match es.subscribe("log") {
        Ok(s) => s,
        Err(e) => {
            update_status.set(format!("Failed to subscribe to log events: {:?}", e));
            es.close();
            update_in_progress.set(false);
            return;
        }
    };

    let mut status_stream = match es.subscribe("status") {
        Ok(s) => s,
        Err(e) => {
            update_status.set(format!("Failed to subscribe to status events: {:?}", e));
            es.close();
            update_in_progress.set(false);
            return;
        }
    };

    let mut update_succeeded = false;

    loop {
        let next_log = log_stream.next();
        let next_status = status_stream.next();
        futures_util::pin_mut!(next_log, next_status);

        match futures_util::future::select(next_log, next_status).await {
            futures_util::future::Either::Left((Some(Ok((_, msg))), _)) => {
                let data = msg.data().as_string().unwrap_or_default();
                if let Ok(payload) = serde_json::from_str::<LogPayload>(&data) {
                    update_status.set(payload.line);
                }
            }
            futures_util::future::Either::Right((Some(Ok((_, msg))), _)) => {
                let data = msg.data().as_string().unwrap_or_default();
                if let Ok(payload) = serde_json::from_str::<StatusPayload>(&data) {
                    match payload.status.as_str() {
                        "succeeded" => {
                            update_status.set("Updated! Restarting...".to_string());
                            update_succeeded = true;
                            break;
                        }
                        "failed" => {
                            let err_msg = payload.error.unwrap_or_else(|| "Unknown error".into());
                            update_status.set(format!("Update failed: {}", err_msg));
                            update_in_progress.set(false);
                            break;
                        }
                        "restarting" => {
                            update_status.set("Restarting Koji...".to_string());
                            update_succeeded = true;
                            break;
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                // Stream ended unexpectedly
                break;
            }
        }
    }

    es.close();

    // If update succeeded, poll for server restart
    if update_succeeded {
        poll_for_restart(
            update_status,
            update_in_progress,
            update_available,
            current_version,
            latest_version,
        )
        .await;
    }
}

/// Poll `/api/self-update/check` every 2 seconds until the server
/// responds with a new version, or give up after 5 attempts.
async fn poll_for_restart(
    update_status: RwSignal<String>,
    update_in_progress: RwSignal<bool>,
    update_available: RwSignal<bool>,
    current_version: RwSignal<String>,
    latest_version: RwSignal<String>,
) {
    let old_version = current_version.get_untracked();
    let max_attempts = 5;

    for attempt in 0..max_attempts {
        gloo_timers::future::TimeoutFuture::new(2_000).await;

        update_status.set(format!(
            "Waiting for server to restart... ({}/{})",
            attempt + 1,
            max_attempts
        ));

        if let Ok(resp) = gloo_net::http::Request::get("/api/self-update/check")
            .send()
            .await
        {
            if let Ok(data) = resp.json::<serde_json::Value>().await {
                if let Some(new_ver) = data["current_version"].as_str() {
                    if new_ver != old_version {
                        update_status.set(format!("Updated to v{}!", new_ver));
                        current_version.set(new_ver.to_string());
                        latest_version.set(String::new());
                        update_available.set(false);
                        update_in_progress.set(false);
                        return;
                    }
                }
            }
        }
    }

    // Gave up — tell user to refresh manually
    update_status.set("Server is restarting. Please refresh the page.".to_string());
    update_in_progress.set(false);
}
