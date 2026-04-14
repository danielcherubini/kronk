use crate::components::pull_wizard::JobProgress;
use leptos::prelude::*;

#[component]
pub fn DoneStep(
    download_jobs: Signal<Vec<JobProgress>>,
    on_close: Option<Callback<()>>,
) -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"All Downloads Complete!"</h2>
            <p class="form-card__desc text-muted">"The following models are now available."</p>
        </div>

        <div class="download-jobs mt-2">
            {move || download_jobs.get().into_iter().map(|job| {
                let badge_class = if job.status == "completed" {
                    "badge badge-success"
                } else {
                    "badge badge-error"
                };
                view! {
                    <div class="flex-between card mb-1 p-cell">
                        <span class="text-mono text-sm">{job.filename}</span>
                        <span class=badge_class>
                            {if job.status == "completed" { "Done ✓" } else { "Failed" }}
                        </span>
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>

        <div class="form-actions mt-3">
            {match on_close {
                Some(cb) => view! {
                    <button type="button" class="btn btn-primary" on:click=move |_| cb.run(())>
                        "Close"
                    </button>
                }.into_any(),
                None => view! {
                    <a href="/models">
                        <button class="btn btn-primary">"View Models →"</button>
                    </a>
                }.into_any(),
            }}
        </div>
    }
}
