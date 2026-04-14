use crate::components::pull_wizard::*;

#[component]
pub fn RepoInput(
    repo_id: RwSignal<String>,
    error_msg: RwSignal<Option<String>>,
    on_close: Option<Callback<()>>,
    on_search: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Enter Repository"</h2>
            <p class="form-card__desc text-muted">
                "Enter a HuggingFace repo ID to search for available quantisations."
            </p>
        </div>

        {move || error_msg.get().map(|e| view! {
            <div class="alert alert--error mb-2">
                <span class="alert__icon">"✕"</span>
                <span>{e}</span>
            </div>
        })}

        <div class="form-group">
            <label class="form-label" for="repo-id">"Repo ID"</label>
            <input
                id="repo-id"
                class="form-input"
                type="text"
                prop:value=move || repo_id.get()
                on:input=move |e| repo_id.set(event_target_value(&e))
                placeholder="e.g. meta-llama/Llama-3.2-1B"
            />
        </div>

        <div class="form-actions mt-3">
            {on_close.map(|cb| view! {
                <button type="button" class="btn btn-secondary" on:click=move |_| cb.run(())>
                    "Cancel"
                </button>
            })}
            <button
                class="btn btn-primary"
                on:click=move |_| {
                    let rid = repo_id.get();
                    if !rid.is_empty() {
                        on_search.run(rid);
                    }
                }
            >
                "Search"
            </button>
        </div>
    }
}
