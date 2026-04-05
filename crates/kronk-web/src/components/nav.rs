use leptos::prelude::*;

#[component]
pub fn Nav() -> impl IntoView {
    view! {
        <nav class="topbar">
            <span class="logo">"⚡ Kronk"</span>
            <a href="/" class="nav-link">"Dashboard"</a>
            <a href="/models" class="nav-link">"Models"</a>
            <a href="/pull" class="nav-link">"Pull Model"</a>
            <a href="/logs" class="nav-link">"Logs"</a>
            <a href="/config" class="nav-link">"Config"</a>
        </nav>
    }
}
