use leptos::prelude::*;

#[component]
pub fn LoadingStep() -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Searching HuggingFace..."</h2>
        </div>
        <div class="spinner-container">
            <span class="spinner"></span>
            <span class="text-muted">"Fetching available quants, please wait..."</span>
        </div>
    }
}
