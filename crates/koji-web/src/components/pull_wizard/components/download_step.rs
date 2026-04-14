use crate::components::pull_wizard::{format_bytes, JobProgress};
use leptos::prelude::*;

#[component]
pub fn DownloadStep(
    download_jobs: Signal<Vec<JobProgress>>,
    on_done: Callback<()>,
    on_close: Option<Callback<()>>,
    error_msg: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Downloading"</h2>
            <p class="form-card__desc text-muted">"Tracking download progress for each file."</p>
        </div>

        {move || error_msg.get().map(|e| view! {
            <div class="alert alert--error mb-2">
                <span class="alert__icon">"✕"</span>
                <span>{e}</span>
            </div>
        })}

        <div class="download-jobs">
            {move || download_jobs.get().into_iter().map(|job| {
                let progress_pct = job.total_bytes
                    .filter(|&total| total > 0)
                    .map(|total| (job.bytes_downloaded as f64 / total as f64 * 100.0) as u32);

                let (status_class, status_text) = if job.status == "completed" {
                    ("badge badge-success", "Completed ✓".to_string())
                } else if job.status == "failed" {
                    ("badge badge-error", format!("Failed: {}", job.error.clone().unwrap_or_default()))
                } else if job.status == "running" {
                    let dl = format_bytes(job.bytes_downloaded as i64);
                    let total = job.total_bytes
                        .map(|b| format_bytes(b as i64))
                        .unwrap_or_else(|| "?".to_string());
                    ("badge badge-info", format!("{dl} / {total}"))
                } else {
                    ("badge badge-muted", job.status.clone())
                };

                view! {
                    <div class="download-job-card card mb-2">
                        <div class="flex-between mb-1">
                            <span class="text-mono text-sm">{job.filename.clone()}</span>
                            <span class=status_class>{status_text}</span>
                        </div>
                        <div class="progress-bar">
                            {if let Some(pct) = progress_pct {
                                view! {
                                    <div
                                        class="progress-bar-fill"
                                        style=format!("width:{}%", pct)
                                    />
                                }.into_any()
                            } else {
                                view! {
                                    <div class="progress-bar-fill indeterminate" />
                                }.into_any()
                            }}
                        </div>
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>

        {move || {
            let jobs = download_jobs.get();
            let all_finished = !jobs.is_empty() && jobs.iter().all(|j| {
                j.status == "completed" || j.status == "failed"
            });
            if !all_finished {
                return None;
            }
            let any_failed = jobs.iter().any(|j| j.status == "failed");
            if any_failed {
                let failures: Vec<_> = jobs.iter()
                    .filter(|j| j.status == "failed")
                    .map(|j| format!(
                        "{}: {}",
                        j.filename,
                        j.error.as_deref().unwrap_or("unknown error")
                    ))
                    .collect();
                Some(view! {
                    <div class="alert alert--error mt-3">
                        <span class="alert__icon">"✕"</span>
                        <div>
                            <p>"Some downloads failed:"</p>
                            <ul class="error-list">
                                {failures.into_iter().map(|msg| view! {
                                    <li>{msg}</li>
                                }).collect::<Vec<_>>()}
                            </ul>
                            <a href="/models" class="mt-2 d-inline">"Go to Models →"</a>
                        </div>
                    </div>
                }.into_any())
            } else {
                 on_done.run(());
                 Some(view! {
                     <div class="alert alert--success mt-3">
                         <span class="alert__icon">"✓"</span>
                         <div>
                             <p>"All downloads completed successfully!"</p>
                             <a href="/models" class="mt-1 d-inline">"Go to Models →"</a>
                         </div>
                     </div>
                 }.into_any())
             }
        }}
        {on_close.map(|cb| view! {
            <div class="form-actions mt-3">
                <button type="button" class="btn btn-secondary" on:click=move |_| cb.run(())>
                    "Hide"
                </button>
            </div>
        })}
    }
}
