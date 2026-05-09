use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use crate::components::modal::Modal;
use crate::components::model_card::ModelCard;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
use crate::components::sparkline::{format_relative_time, SparklineChart};
use crate::utils::{post_request, rw_signal_to_signal};

mod metrics;
pub use metrics::*;

#[cfg(test)]
mod tests;

#[component]
pub fn Dashboard() -> impl IntoView {
    let history = RwSignal::new(Vec::<MetricSample>::new());
    let fetch_failed = RwSignal::new(false);
    // Incrementing this signal re-runs the Effect that opens the EventSource.
    let connect_trigger = RwSignal::new(0u32);

    // Open (or re-open) an EventSource each time connect_trigger changes.
    Effect::new(move |_| {
        let _ = connect_trigger.get(); // track signal

        let es = match web_sys::EventSource::new("/tama/v1/system/metrics/stream") {
            Ok(es) => es,
            Err(_) => {
                fetch_failed.set(true);
                return;
            }
        };

        // Handler for "snapshot" events — replaces the entire history buffer.
        let on_snapshot =
            Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    if let Ok(samples) = serde_json::from_str::<Vec<MetricSample>>(&data_str) {
                        fetch_failed.set(false);
                        history.set(samples);
                    }
                }
            });
        let _ =
            es.add_event_listener_with_callback("snapshot", on_snapshot.as_ref().unchecked_ref());
        on_snapshot.forget();

        // Error handler — flag for the empty-history retry UI.
        let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
            fetch_failed.set(true);
        });
        es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        // Close the EventSource when the effect re-runs or the component unmounts.
        on_cleanup(move || {
            es.close();
        });
    });

    // Manual retry: close and re-open the EventSource.
    let manual_refresh = move |_| {
        fetch_failed.set(false);
        connect_trigger.update(|n| *n += 1);
    };

    let restart: Action<(), (), LocalStorage> = Action::new_unsync(|_: &()| async move {
        let _ = post_request("/tama/v1/system/restart").send().await;
    });

    // Per-model load/unload actions wired to the same REST endpoints used by
    // the `/models` page. Both actions are unsync because `gloo_net::Request`
    // returns `!Send` futures in the WASM target.
    //
    // We use a manual "busy" signal instead of relying on Action::pending()
    // because in some WASM error scenarios (e.g. proxy returns 500 with no
    // backend configured), the pending flag can get stuck and never reset,
    // leaving buttons permanently disabled with "Loading…" text.
    let load_busy = RwSignal::new(false);
    let unload_busy = RwSignal::new(false);

    // Pull Model modal
    let pull_modal_open = RwSignal::new(false);

    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            load_busy.set(true);
            // Ignore errors — the SSE stream will push updated model state.
            // Even if the request fails (e.g. no backend configured), we set
            // load_busy to false below so the button becomes clickable again.
            let _ = post_request(&format!("/tama/v1/models/{}/load", id))
                .send()
                .await;
            load_busy.set(false);
        }
    });
    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            unload_busy.set(true);
            // Same as load — ignore errors, SSE will push the updated state.
            let _ = post_request(&format!("/tama/v1/models/{}/unload", id))
                .send()
                .await;
            unload_busy.set(false);
        }
    });

    view! {
        <div class="page-header">
            <h1>"Dashboard"</h1>
            <div class="page-header-actions">
                // Existing status badge + Restart (inside conditional, only shown after SSE data arrives)
                {move || {
                    history.get().last().cloned().map(|_h| {
                        let badge_class = if fetch_failed.get() { "badge badge-danger" } else { "badge badge-success" };
                        let badge_text = if fetch_failed.get() { "error" } else { "ok" };
                        view! {
                            <div class="flex-between gap-1">
                                <span class={badge_class}>{badge_text}</span>
                                <button class="btn btn-secondary" on:click=move |_| { restart.dispatch(()); }>
                                    "Restart"
                                </button>
                            </div>
                        }
                    })
                }}
                // New buttons (always visible, outside conditional)
                <button class="btn btn-secondary" on:click=move |_| pull_modal_open.set(true)>"Pull Model"</button>

            </div>
        </div>



        {move || {
            let buf = history.get();
            if fetch_failed.get() && buf.is_empty() {
                // Network error, no data yet — show error with retry button
                return view! {
                    <div class="card">
                        <p class="text-error">"Failed to load metrics stream. Is Tama running?"</p>
                        <button class="btn btn-secondary btn-sm mt-2" on:click=manual_refresh>"Retry"</button>
                    </div>
                }.into_any();
            }

            // Extract data for sparkline charts
            let cpu_data: Vec<f32> = buf.iter().map(|s| s.cpu_usage_pct).collect();
            let mem_data: Vec<f32> = buf.iter().map(|s| s.ram_used_mib as f32).collect();
            let timestamps: Vec<i64> = buf.iter().map(|s| s.ts_unix_ms).collect();
            let mem_max = buf.last().map(|h| h.ram_total_mib as f32).unwrap_or(1.0);
            let cpu_y_refs = vec![0.0, 100.0];
            let mem_y_refs = vec![mem_max];

            let gpu_data: Vec<f32> = buf.iter().map(|s| s.gpu_utilization_pct.unwrap_or(0) as f32).collect();
            let vram_data: Vec<f32> = buf.iter().map(|s| s.vram.as_ref().map(|v| v.used_mib as f32).unwrap_or(0.0)).collect();
            let vram_max = buf.last().and_then(|h| h.vram.as_ref().map(|v| v.total_mib as f32)).unwrap_or(1.0);
            let vram_y_refs = vec![vram_max];

            let all_models: Vec<ModelStatus> = buf.last().map(|h| h.models.clone()).unwrap_or_default();
            let active = active_models(&all_models);
            let inactive = inactive_models(&all_models);

            view! {
                <div class="grid-stats">
                    // CPU card
                    <div class="stat-card">
                        <div class="card-header">"CPU Usage"</div>
                        {match buf.last() {
                            Some(h) => view! {
                                <div class="card-value">{format!("{:.1}%", h.cpu_usage_pct)}</div>
                                <div class="card-secondary">"of 100%"</div>
                            }.into_any(),
                            None => view! {
                                <div class="card-value-empty">"—"</div>
                            }.into_any(),
                        }}
                        <div class="sparkline-container">
                            <SparklineChart
                                data=cpu_data
                                max_value=100.0
                                color="var(--accent-green)".to_string()
                                height=60.0
                                timestamps=timestamps.clone()
                                unit_label="%".to_string()
                                y_refs=cpu_y_refs
                            />
                        </div>
                    </div>

                    // Memory card
                    <div class="stat-card">
                        <div class="card-header">"Memory"</div>
                        {match buf.last() {
                            Some(h) => view! {
                                <div class="card-value">{format_number(h.ram_used_mib)}</div>
                                <div class="card-secondary">{format!("of {} MiB", format_number(h.ram_total_mib))}</div>
                            }.into_any(),
                            None => view! {
                                <div class="card-value-empty">"—"</div>
                            }.into_any(),
                        }}
                        <div class="sparkline-container">
                            <SparklineChart
                                data=mem_data
                                max_value=mem_max
                                color="var(--accent-blue)".to_string()
                                height=60.0
                                timestamps=timestamps.clone()
                                unit_label="MiB".to_string()
                                y_refs=mem_y_refs
                            />
                        </div>
                    </div>

                    // GPU card — only rendered if GPU data is present
                    {if let Some(gpu_pct) = buf.last().and_then(|h| h.gpu_utilization_pct) {
                        view! {
                            <div class="stat-card">
                                <div class="card-header">"GPU"</div>
                                <div class="card-value">{format!("{}%", gpu_pct)}</div>
                                <div class="card-secondary">"of 100%"</div>
                                <div class="sparkline-container">
                                    <SparklineChart
                                        data=gpu_data
                                        max_value=100.0
                                        color="var(--accent-yellow)".to_string()
                                        height=60.0
                                        timestamps=timestamps.clone()
                                        unit_label="%".to_string()
                                        y_refs=vec![0.0_f32, 100.0_f32]
                                    />
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }}

                    // VRAM card — only rendered if VRAM data is present
                    {if let Some(vram_info) = buf.last().and_then(|h| h.vram.as_ref()) {
                        view! {
                            <div class="stat-card">
                                <div class="card-header">"VRAM"</div>
                                <div class="card-value">{format_number(vram_info.used_mib)}</div>
                                <div class="card-secondary">{format!("of {} MiB", format_number(vram_info.total_mib))}</div>
                                <div class="sparkline-container">
                                    <SparklineChart
                                        data=vram_data
                                        max_value=vram_max
                                        color="var(--accent-purple)".to_string()
                                        height=60.0
                                        timestamps=timestamps
                                        unit_label="MiB".to_string()
                                        y_refs=vram_y_refs
                                    />
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }}
                </div>

                // Inference stats cards — always visible, show "—" until data arrives
                {move || {
                    let buf = history.get();
                    let latest = buf.last();

                    // Compute "time since" string for inference stats cards
                    let inference_time_ago = latest
                        .and_then(|s| s.inference_last_updated_ms)
                        .map(format_relative_time)
                        .unwrap_or_default();

                    // Extract sparkline data from ALL samples (full 15-min window),
                    // filling in 0.0 for samples where inference hasn't been observed yet.
                    let timestamps: Vec<i64> = buf.iter().map(|s| s.ts_unix_ms).collect();
                    let tps_data: Vec<f32> = buf.iter()
                        .map(|s| s.tps.unwrap_or(0.0)).collect();
                    let prompt_tps_data: Vec<f32> = buf.iter()
                        .map(|s| s.prompt_tps.unwrap_or(0.0)).collect();
                    let cache_data: Vec<f32> = buf.iter()
                        .map(|s| s.cache_hit_pct.unwrap_or(0.0)).collect();
                    let spec_data: Vec<f32> = buf.iter()
                        .map(|s| s.spec_accept_pct.unwrap_or(0.0)).collect();

                    // Determine max values for sparkline scaling
                    let tps_max = tps_data.iter().cloned().fold(1.0f32, f32::max);
                    let prompt_tps_max = prompt_tps_data.iter().cloned().fold(1.0f32, f32::max);

                    view! {
                        <div class="grid-stats grid-stats--inference">
                            // Processing Speed card
                            <div class="stat-card">
                                <div class="card-header">"Processing Speed"</div>
                                {match latest.and_then(|s| s.prompt_tps) {
                                    Some(v) => view! {
                                        <div class="card-value">{format!("{:.1} tok/s", v)}</div>
                                        <div class="card-secondary">{inference_time_ago.clone()}</div>
                                    }.into_any(),
                                    None => view! {
                                        <div class="card-value-empty">"—"</div>
                                    }.into_any(),
                                }}
                                <div class="sparkline-container">
                                    <SparklineChart
                                        data=prompt_tps_data
                                        max_value=prompt_tps_max
                                        color="var(--accent-orange)".to_string()
                                        height=60.0
                                        timestamps=timestamps.clone()
                                        unit_label="tok/s".to_string()
                                        y_refs=vec![]
                                    />
                                </div>
                            </div>

                            // Gen Speed card
                            <div class="stat-card">
                                <div class="card-header">"Gen Speed"</div>
                                {match latest.and_then(|s| s.tps) {
                                    Some(v) => view! {
                                        <div class="card-value">{format!("{:.1} tok/s", v)}</div>
                                        <div class="card-secondary">{inference_time_ago.clone()}</div>
                                    }.into_any(),
                                    None => view! {
                                        <div class="card-value-empty">"—"</div>
                                    }.into_any(),
                                }}
                                <div class="sparkline-container">
                                    <SparklineChart
                                        data=tps_data
                                        max_value=tps_max
                                        color="var(--accent-cyan)".to_string()
                                        height=60.0
                                        timestamps=timestamps.clone()
                                        unit_label="tok/s".to_string()
                                        y_refs=vec![]
                                    />
                                </div>
                            </div>

                            // Cache Hits card
                            <div class="stat-card">
                                <div class="card-header">"Cache Hits"</div>
                                {match latest.and_then(|s| s.cache_hit_pct) {
                                    Some(v) => view! {
                                        <div class="card-value">{format!("{:.1}%", v)}</div>
                                        <div class="card-secondary">{inference_time_ago.clone()}</div>
                                    }.into_any(),
                                    None => view! {
                                        <div class="card-value-empty">"—"</div>
                                    }.into_any(),
                                }}
                                <div class="sparkline-container">
                                    <SparklineChart
                                        data=cache_data
                                        max_value=100.0
                                        color="var(--accent-green)".to_string()
                                        height=60.0
                                        timestamps=timestamps.clone()
                                        unit_label="%".to_string()
                                        y_refs=vec![0.0, 100.0]
                                    />
                                </div>
                            </div>

                            // Spec Accept card
                            <div class="stat-card">
                                <div class="card-header">"Spec Accept"</div>
                                {match latest.and_then(|s| s.spec_accept_pct) {
                                    Some(v) => view! {
                                        <div class="card-value">{format!("{:.1}%", v)}</div>
                                        <div class="card-secondary">{inference_time_ago.clone()}</div>
                                    }.into_any(),
                                    None => view! {
                                        <div class="card-value-empty">"—"</div>
                                    }.into_any(),
                                }}
                                <div class="sparkline-container">
                                    <SparklineChart
                                        data=spec_data
                                        max_value=100.0
                                        color="var(--accent-pink)".to_string()
                                        height=60.0
                                        timestamps=timestamps
                                        unit_label="%".to_string()
                                        y_refs=vec![0.0, 100.0]
                                    />
                                </div>
                            </div>
                        </div>
                    }.into_any()
                }}

                // Active Models section
                <section class="dashboard-models">
                    <div class="page-header">
                        <h2>"Active Models"</h2>
                        <span class="text-muted">
                            {format!("{} loaded", active.len())}
                        </span>
                    </div>
                    {
                        if all_models.is_empty() {
                            view! {
                                <div class="card card--centered">
                                    <p class="text-muted">"No models configured yet."</p>
                                </div>
                            }.into_any()
                        } else if active.is_empty() {
                            view! {
                                <div class="card card--centered">
                                    <p class="text-muted">"No models currently loaded."</p>
                                </div>
                            }.into_any()
                        } else {
                            let mut active_sorted = active.clone();
                            active_sorted.sort_by_key(model_sort_key);
                            view! {
                                <div class="models-list">
                                    {active_sorted.into_iter().map(|m| {
                                        let on_load_cb = Callback::new(move |id: String| {
                                            load_action.dispatch(id);
                                        });
                                        let on_unload_cb = Callback::new(move |id: String| {
                                            unload_action.dispatch(id);
                                        });
                                        view! {
                                            <ModelCard
                                                id=m.id.clone()
                                                db_id=m.db_id
                                                display_name=model_display_name(&m)
                                                quant=m.quant.clone()
                                                context_length=m.context_length
                                                hf_architecture_type=m.hf_architecture_type.clone()
                                                hf_base_model=m.hf_base_model.clone()
                                                backend=m.backend.clone()
                                                log_source=Some(format!("{}_{}", m.backend, m.id))
                                                state=m.state.clone()
                                                loaded=None
                                                enabled=None
                                                on_load=on_load_cb
                                                on_unload=on_unload_cb
                                                load_busy=load_busy
                                                unload_busy=unload_busy
                                            />
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        }
                    }
                </section>

                // Inactive Models section — only render when all_models is non-empty
                {if all_models.is_empty() {
                    view! { <div></div> }.into_any()
                } else {
                    view! {
                        <section class="dashboard-models">
                            <div class="page-header">
                                <h2>"Inactive Models"</h2>
                                <span class="text-muted">
                                    {format!("{} inactive", inactive.len())}
                                </span>
                            </div>
                            {
                                if inactive.is_empty() {
                                    view! {
                                        <div class="card card--centered">
                                            <p class="text-muted">"No inactive models."</p>
                                        </div>
                                    }.into_any()
                                } else {
                                    let mut inactive_sorted = inactive.clone();
                                    inactive_sorted.sort_by_key(model_sort_key);
                                    view! {
                                        <div class="models-list">
                                            {inactive_sorted.into_iter().map(|m| {
                                                let on_load_cb = Callback::new(move |id: String| {
                                                    load_action.dispatch(id);
                                                });
                                                let on_unload_cb = Callback::new(move |id: String| {
                                                    unload_action.dispatch(id);
                                                });
                                                view! {
                                                    <ModelCard
                                                        id=m.id.clone()
                                                        db_id=m.db_id
                                                        display_name=model_display_name(&m)
                                                        quant=m.quant.clone()
                                                        context_length=m.context_length
                                                        hf_architecture_type=m.hf_architecture_type.clone()
                                                        hf_base_model=m.hf_base_model.clone()
                                                        backend=m.backend.clone()
                                                        log_source=Some(format!("{}_{}", m.backend, m.id))
                                                        state=m.state.clone()
                                                        loaded=None
                                                        enabled=None
                                                        on_load=on_load_cb
                                                        on_unload=on_unload_cb
                                                        load_busy=load_busy
                                                        unload_busy=unload_busy
                                                    />
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }.into_any()
                                }
                            }
                        </section>
                    }.into_any()
                }}
            }.into_any()
        }}

        <Modal
            open=rw_signal_to_signal(pull_modal_open)
            on_close=Callback::new(move |_| pull_modal_open.set(false))
            title="Pull Model".to_string()
        >
            <PullQuantWizard
                initial_repo=Signal::derive(String::new)
                is_open=rw_signal_to_signal(pull_modal_open)
                on_complete=Callback::new(move |_completed: Vec<CompletedQuant>| {
                    pull_modal_open.set(false);
                    connect_trigger.update(|n| *n += 1);
                })
                on_close=Callback::new(move |_| pull_modal_open.set(false))
            />
        </Modal>
    }
}
