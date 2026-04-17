use leptos::prelude::*;
use leptos_router::components::A;
use web_sys::window;

#[component]
pub fn Sidebar() -> impl IntoView {
    let collapsed = RwSignal::new(false);
    let mobile_open = RwSignal::new(false);
    let update_badge_visible = RwSignal::new(false);

    // Check for backend/model updates on mount (separate from self-update)
    Effect::new(move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/api/updates").send().await {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    let has_updates = data
                        .get("backends")
                        .and_then(|b| b.as_array())
                        .map(|arr| {
                            arr.iter().any(|b| {
                                b.get("update_available")
                                    .and_then(|u| u.as_bool())
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false)
                        || data
                            .get("models")
                            .and_then(|m| m.as_array())
                            .map(|arr| {
                                arr.iter().any(|m| {
                                    m.get("update_available")
                                        .and_then(|u| u.as_bool())
                                        .unwrap_or(false)
                                })
                            })
                            .unwrap_or(false);
                    update_badge_visible.set(has_updates);
                }
            }
        });
    });

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
                <A href="/updates" attr:class="sidebar-item" attr:data-tooltip="Updates" on:click=move |_| mobile_open.set(false)>
                    <span class="sidebar-item__icon">"🔄"</span>
                    <span class="sidebar-item__text">"Updates"</span>
                    {move || update_badge_visible.get().then(|| view! {
                        <span class="sidebar-badge">"!"</span>
                    })}
                </A>
            </nav>

            <div class="sidebar-footer">
                <div class="sidebar-section" style="border-top:none;margin:0;padding:0;">
                    <A href="/config" attr:class="sidebar-item" attr:data-tooltip="Config" on:click=move |_| mobile_open.set(false)>
                        <span class="sidebar-item__icon">"⚙️"</span>
                        <span class="sidebar-item__text">"Config"</span>
                    </A>
                </div>



                <button class="sidebar-toggle" on:click=move |_| collapsed.update(|c| *c = !*c)>
                    <span class="sidebar-toggle__icon">"↔"</span>
                    <span class="sidebar-toggle__text">"Collapse"</span>
                </button>
            </div>
        </aside>
    }
}
