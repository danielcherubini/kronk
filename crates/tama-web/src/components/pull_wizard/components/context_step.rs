use crate::components::context_length_selector::ContextLengthSelector;
use crate::components::pull_wizard::*;

#[component]
pub fn ContextStep(
    /// GGUF-parsed context length (native max for the model).
    gguf_context_length: Signal<Option<u64>>,
    /// Downloaded quant files (model quants only, no mmproj).
    download_jobs: Signal<Vec<JobProgress>>,
    /// The settings the user configures.
    settings: RwSignal<ContextSettings>,
    on_next: Callback<()>,
    on_back: Callback<()>,
) -> impl IntoView {
    // Pre-fill context_length from GGUF if not already set
    Effect::new(move |_| {
        if settings.get().context_length.is_none() {
            if let Some(gguf_ctx) = gguf_context_length.get() {
                settings.update(|s| {
                    s.context_length = Some(gguf_ctx as u32);
                });
            }
        }
    });

    // Max context for the dropdown (capped at GGUF native value)
    let max_context = Signal::derive(move || gguf_context_length.get().unwrap_or(262144) as u32);

    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Configure Model"</h2>
            <p class="form-card__desc text-muted">
                "Set context length and KV cache settings for this model."
            </p>
        </div>

        // ── Section A: Context Length ──────────────────────────────────────
        <div class="form-section mb-4">
            <h3 class="form-label">"Context Length"</h3>
            <p class="text-muted text-sm mb-2">
                {move || {
                    if let Some(native) = gguf_context_length.get() {
                        format!("Native context: {} tokens. Set lower to use less RAM.", native)
                    } else {
                        "Set the context window size. Higher values use more RAM.".to_string()
                    }
                }}
            </p>
            <ContextLengthSelector
                class="input-narrow".to_string()
                value=Signal::derive(move || settings.get().context_length)
                on_change=Callback::new(move |v| {
                    settings.update(|s| s.context_length = v);
                })
                reset_key=Signal::derive(move || "wizard-context".to_string())
                max_context=Signal::derive(move || Some(max_context.get()))
            />
        </div>

        // ── Section B: KV Cache Quantization ───────────────────────────────
        <div class="form-section mb-4">
            <h3 class="form-label">"KV Cache Quantization"</h3>
            <p class="text-muted text-sm mb-2">
                "Quantize the KV cache to reduce memory usage. Leave as default (none) for best quality."
            </p>

            <div class="form-group mb-2">
                <label class="form-label text-sm">"Unified K/V Cache"</label>
                <label class="toggle">
                    <input
                        type="checkbox"
                        prop:checked=move || settings.get().kv_unified
                        on:change=move |e| {
                            settings.update(|s| s.kv_unified = event_target_checked(&e));
                        }
                    />
                    <span class="toggle-slider"></span>
                </label>
            </div>

            <div class="form-group mb-2">
                <label class="form-label text-sm">"K Cache Type"</label>
                <select
                    class="form-select input-narrow"
                    prop:value=move || settings.get().cache_type_k.clone().unwrap_or_default()
                    on:change=move |e| {
                        let v = crate::utils::target_value(&e);
                        settings.update(|s| {
                            s.cache_type_k = if v.is_empty() { None } else { Some(v) };
                        });
                    }
                >
                    <option value="">"Default (none)"</option>
                    {KV_QUANT_OPTIONS.iter().map(|opt| {
                        let opt_str = *opt;
                        view! { <option value=opt_str>{opt_str}</option> }
                    }).collect::<Vec<_>>()}
                </select>
            </div>

            <Show when=move || !settings.get().kv_unified>
                <div class="form-group mb-2">
                    <label class="form-label text-sm">"V Cache Type"</label>
                    <select
                        class="form-select input-narrow"
                        prop:value=move || settings.get().cache_type_v.clone().unwrap_or_default()
                        on:change=move |e| {
                            let v = crate::utils::target_value(&e);
                            settings.update(|s| {
                                s.cache_type_v = if v.is_empty() { None } else { Some(v) };
                            });
                        }
                    >
                        <option value="">"Default (none)"</option>
                        {KV_QUANT_OPTIONS.iter().map(|opt| {
                            let opt_str = *opt;
                            view! { <option value=opt_str>{opt_str}</option> }
                        }).collect::<Vec<_>>()}
                    </select>
                </div>
            </Show>
        </div>

        // ── Downloaded files summary ───────────────────────────────────────
        // Filter out mmproj files — this step is for model config only.
        <div class="form-section mb-3">
            <h3 class="form-label">"Downloaded Files"</h3>
            <div class="download-summary">
                {move || {
                    download_jobs.get().iter()
                        .filter(|job| !job.filename.starts_with("mmproj"))
                        .map(|job| {
                        let badge_class = if job.status == "completed" {
                            "badge badge-success"
                        } else {
                            "badge badge-error"
                        };
                        view! {
                            <div class="flex-between mb-1">
                                <span class="text-mono text-sm">{job.filename.clone()}</span>
                                <span class=badge_class>
                                    {if job.status == "completed" { "Done ✓" } else { "Failed" }}
                                </span>
                            </div>
                        }
                    }).collect::<Vec<_>>()
                }}
            </div>
        </div>

        <div class="form-actions mt-3">
            <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                "Back"
            </button>
            <button class="btn btn-primary" on:click=move |_| on_next.run(())>
                "Save & Finish"
            </button>
        </div>
    }
}
