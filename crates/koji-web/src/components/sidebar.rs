use leptos::prelude::*;
use leptos_router::components::A;
use web_sys::window;

#[component]
pub fn Sidebar() -> impl IntoView {
    let collapsed = RwSignal::new(false);

    // On mount, read localStorage for persisted state.
    // Use a plain closure (not Effect::new) since this has no reactive dependencies.
    let initial = window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|ls| ls.get("koji-sidebar-collapsed").ok())
        .flatten();
    if initial.as_deref() == Some("true") {
        collapsed.set(true);
    }

    // Persist state when it changes — this IS reactive (subscribes to collapsed).
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
        <aside class="sidebar" class:sidebar--collapsed=move || collapsed.get()>
            <A href="/" attr:class="sidebar-header">
                <span class="sidebar-header__logo">"⚡"</span>
                <span class="sidebar-header__text">"Koji"</span>
            </A>

            <nav class="sidebar-nav">
                <A href="/" attr:class="sidebar-item" attr:data-tooltip="Dashboard">
                    <span class="sidebar-item__icon">"🏠"</span>
                    <span class="sidebar-item__text">"Dashboard"</span>
                </A>
                <A href="/models" attr:class="sidebar-item" attr:data-tooltip="Models">
                    <span class="sidebar-item__icon">"📦"</span>
                    <span class="sidebar-item__text">"Models"</span>
                </A>
                <A href="/backends" attr:class="sidebar-item" attr:data-tooltip="Backends">
                    <span class="sidebar-item__icon">"🔧"</span>
                    <span class="sidebar-item__text">"Backends"</span>
                </A>
                <A href="/logs" attr:class="sidebar-item" attr:data-tooltip="Logs">
                    <span class="sidebar-item__icon">"📋"</span>
                    <span class="sidebar-item__text">"Logs"</span>
                </A>
            </nav>

            <div class="sidebar-footer">
                <div class="sidebar-section" style="border-top:none;margin:0;padding:0;">
                    <A href="/config" attr:class="sidebar-item" attr:data-tooltip="Config">
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
