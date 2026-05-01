use crate::components::context_length_selector::ContextLengthSelector;
use crate::components::pull_wizard::*;

/// Simple context length selector for the pull wizard.
#[component]
pub fn ContextStep(
    context_length: RwSignal<u32>,
    on_next: Callback<()>,
    on_back: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Set Context Length"</h2>
            <p class="form-card__desc text-muted">
                "Choose the context window size for this model."
            </p>
        </div>

        <div class="mb-3">
            <label class="form-label">"Context Length"</label>
            <ContextLengthSelector
                class="input-narrow".to_string()
                value=Signal::derive(move || Some(context_length.get()))
                on_change=Callback::new(move |v: Option<u32>| {
                    let val = v.unwrap_or(32768);
                    context_length.set(val);
                })
                reset_key=Signal::derive(move || "wizard-static".to_string())
            />
        </div>

        <div class="form-actions mt-3">
            <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                "Back"
            </button>
            <button class="btn btn-primary" on:click=move |_| on_next.run(())>
                "Next →"
            </button>
        </div>
    }
}
