use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn Nav() -> impl IntoView {
    view! {
        <nav class="topbar">
            <span class="logo">"⚡ Koji"</span>
            <A href="/" attr:class="nav-link">"Dashboard"</A>
            <A href="/models" attr:class="nav-link">"Models"</A>
            <A href="/logs" attr:class="nav-link">"Logs"</A>
            <A href="/config" attr:class="nav-link">"Config"</A>
        </nav>
    }
}
