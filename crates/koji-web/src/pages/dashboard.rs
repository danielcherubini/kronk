use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::components::sparkline::SparklineChart;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetricSample {
    ts_unix_ms: i64,
    cpu_usage_pct: f32,
    ram_used_mib: u64,
    ram_total_mib: u64,
    gpu_utilization_pct: Option<u8>,
    vram: Option<VramInfo>,
    models_loaded: u64,
    /// Per-model loaded/idle status mirrored from `koji_core::gpu::MetricSample.models`.
    ///
    /// `#[serde(default)]` keeps the dashboard resilient if the backend is
    /// slightly out of sync (e.g. during a partial rollout) or if older cached
    /// payloads without this field are encountered — missing arrays decode as
    /// an empty `Vec` rather than failing the whole sample.
    #[serde(default)]
    pub models: Vec<ModelStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VramInfo {
    used_mib: u64,
    total_mib: u64,
}

/// Frontend mirror of the backend `MetricsHistoryEntry` response type.
///
/// Uses `i64` for memory and GPU fields to match the JSON wire format
/// (SQLite stores integers as i64). Converted to `MetricSample` on ingestion.
#[derive(Debug, Clone, Deserialize)]
struct MetricsHistoryEntry {
    ts_unix_ms: i64,
    cpu_usage_pct: f32,
    ram_used_mib: i64,
    ram_total_mib: i64,
    gpu_utilization_pct: Option<i64>,
    vram_used_mib: Option<i64>,
    vram_total_mib: Option<i64>,
}

impl From<MetricsHistoryEntry> for MetricSample {
    fn from(entry: MetricsHistoryEntry) -> Self {
        MetricSample {
            ts_unix_ms: entry.ts_unix_ms,
            cpu_usage_pct: entry.cpu_usage_pct,
            ram_used_mib: entry.ram_used_mib as u64,
            ram_total_mib: entry.ram_total_mib as u64,
            gpu_utilization_pct: entry.gpu_utilization_pct.map(|v| v as u8),
            vram: entry.vram_used_mib.and_then(|used| {
                entry.vram_total_mib.map(|total| VramInfo {
                    used_mib: used as u64,
                    total_mib: total as u64,
                })
            }),
            models_loaded: 0,
            models: vec![],
        }
    }
}

/// Frontend mirror of `koji_core::gpu::ModelStatus`.
///
/// Kept private to this module so the dashboard owns its wire shape; the only
/// contract with the backend is the JSON field names, which must match the
/// server-side struct exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelStatus {
    id: String,
    #[serde(default)]
    db_id: Option<i64>,
    #[serde(default)]
    api_name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    backend: String,
    loaded: bool,
}

/// Format a number with comma separators (e.g. `8460` → `"8,460"`).
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, c);
    }
    result
}

/// Count how many models in `models` are currently loaded.
///
/// Extracted as a free function so the dashboard view and the unit tests can
/// share a single implementation — the view formats the result into the
/// "Active Models" summary line, and the tests assert the counting logic
/// without needing to render Leptos components.
fn loaded_model_count(models: &[ModelStatus]) -> usize {
    models.iter().filter(|m| m.loaded).count()
}

/// CSS class string used for the per-model status badge in the
/// "Active Models" grid. Loaded models get the success colour, idle ones
/// get the muted colour. Extracted so the rendering branch and the unit
/// tests share a single source of truth.
fn model_status_badge_class(loaded: bool) -> &'static str {
    if loaded {
        "badge badge-success"
    } else {
        "badge badge-muted"
    }
}

/// Human-readable label that pairs with [`model_status_badge_class`].
fn model_status_badge_label(loaded: bool) -> &'static str {
    if loaded {
        "Loaded"
    } else {
        "Idle"
    }
}

/// CSS class string for the load/unload action button in a model card.
/// Loaded models render a destructive "Unload" button (`btn-danger`),
/// idle models render an affirmative "Load" button (`btn-success`).
fn model_action_button_class(loaded: bool) -> &'static str {
    if loaded {
        "btn btn-danger btn-sm"
    } else {
        "btn btn-success btn-sm"
    }
}

/// Human-readable label that pairs with [`model_action_button_class`].
fn model_action_button_label(loaded: bool) -> &'static str {
    if loaded {
        "Unload"
    } else {
        "Load"
    }
}

/// Returns the preferred display name for a model, preferring `display_name`,
/// then `api_name`, falling back to the model `id` otherwise.
fn model_display_name(m: &ModelStatus) -> String {
    m.display_name
        .as_deref()
        .or(m.api_name.as_deref())
        .unwrap_or(m.id.as_str())
        .to_string()
}

/// Partition model statuses into loaded and unloaded vectors, sorted by ID.
///
/// This is extracted to avoid duplicating the partition logic between the
/// "Loaded Models" and "Idle Models" sections.
fn partition_model_statuses(models: Vec<ModelStatus>) -> (Vec<ModelStatus>, Vec<ModelStatus>) {
    let (mut loaded, mut unloaded): (Vec<_>, Vec<_>) = models.into_iter().partition(|m| m.loaded);
    loaded.sort_by(|a, b| a.id.cmp(&b.id));
    unloaded.sort_by(|a, b| a.id.cmp(&b.id));
    (loaded, unloaded)
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let history = RwSignal::new(Vec::<MetricSample>::new());
    let fetch_failed = RwSignal::new(false);
    // Incrementing this signal re-runs the Effect that opens the EventSource.
    let connect_trigger = RwSignal::new(0u32);

    // Fetch historical metrics on mount, before connecting to SSE.
    // This populates the chart with up to 450 recent data points (15 minutes at 2s intervals).
    {
        let history_signal = history;
        spawn_local(async move {
            if let Ok(resp) =
                gloo_net::http::Request::get("/koji/v1/system/metrics/history?limit=450")
                    .send()
                    .await
            {
                if let Ok(entries) = resp.json::<Vec<MetricsHistoryEntry>>().await {
                    let samples: Vec<MetricSample> = entries.into_iter().map(Into::into).collect();
                    if !samples.is_empty() {
                        history_signal.update(|buf| {
                            *buf = samples;
                        });
                    }
                }
            }
        });
    }

    // Open (or re-open) an EventSource each time connect_trigger changes.
    Effect::new(move |_| {
        let _ = connect_trigger.get(); // track signal

        let es = match web_sys::EventSource::new("/koji/v1/system/metrics/stream") {
            Ok(es) => es,
            Err(_) => {
                fetch_failed.set(true);
                return;
            }
        };

        // Handler for "sample" events.
        let on_sample =
            Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    if let Ok(sample) = serde_json::from_str::<MetricSample>(&data_str) {
                        fetch_failed.set(false);
                        history.update(|buf| {
                            buf.push(sample);
                            if buf.len() > 450 {
                                buf.drain(..buf.len() - 450);
                            }
                        });
                    }
                }
            });
        let _ = es.add_event_listener_with_callback("sample", on_sample.as_ref().unchecked_ref());
        on_sample.forget();

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
        let _ = gloo_net::http::Request::post("/koji/v1/system/restart")
            .send()
            .await;
    });

    // Per-model load/unload actions wired to the same REST endpoints used by
    // the `/models` page. Both actions are unsync because `gloo_net::Request`
    // returns `!Send` futures in the WASM target. We deliberately do **not**
    // refresh anything on completion: the dashboard's SSE stream pushes a new
    // `MetricSample` every tick, so the freshly toggled `loaded` flag flows
    // back into the UI without us having to manage cache invalidation here.
    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(|id: &String| {
        let id = id.clone();
        async move {
            let _ = gloo_net::http::Request::post(&format!("/koji/v1/models/{}/load", id))
                .send()
                .await;
        }
    });
    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(|id: &String| {
        let id = id.clone();
        async move {
            let _ = gloo_net::http::Request::post(&format!("/koji/v1/models/{}/unload", id))
                .send()
                .await;
        }
    });

    // Capture the pending signals once so the per-card buttons can disable
    // themselves while a load/unload request is in flight — this prevents
    // double-clicks from queuing duplicate requests against the proxy.
    let load_pending = load_action.pending();
    let unload_pending = unload_action.pending();

    view! {
        <div class="page-header">
            <h1>"Dashboard"</h1>
            {move || {
                history.get().last().cloned().map(|_h| {
                    let badge_class = if fetch_failed.get() { "badge badge-danger" } else { "badge badge-success" };
                    let badge_text = if fetch_failed.get() { "error" } else { "ok" };
                    view! {
                        <div class="flex-between gap-1">
                            <span class={badge_class}>{badge_text}</span>
                            <button class="btn btn-secondary btn-sm" on:click=move |_| { restart.dispatch(()); }>
                                "Restart"
                            </button>
                        </div>
                    }
                })
            }}
        </div>

        {move || {
            let buf = history.get();
            if fetch_failed.get() && buf.is_empty() {
                // Network error, no data yet — show error with retry button
                return view! {
                    <div class="card">
                        <p class="text-error">"Failed to load metrics stream. Is Koji running?"</p>
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

            let models: Vec<ModelStatus> = buf.last().map(|h| h.models.clone()).unwrap_or_default();

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

                // Active Models section
                <section class="dashboard-models">
                    <div class="page-header">
                        <h2>"Active Models"</h2>
                        <span class="text-muted">
                            {format!("{} loaded", loaded_model_count(&models))}
                        </span>
                    </div>
                    {
                        if models.is_empty() {
                            view! {
                                <div class="card card--centered">
                                    <p class="text-muted">"No models configured yet."</p>
                                </div>
                            }.into_any()
                        } else {
                            // Partition models into loaded and idle sections
                            let (loaded, idle) = partition_model_statuses(models);
                            view! {
                                // Plain wrapper div (NOT `.models-grid`) so the two
                                // `.model-section` children stack vertically, matching
                                // the Models page. The inner `.models-grid` inside each
                                // section is what flows the model cards horizontally.
                                <div>
                                    // Loaded models section
                                    {if !loaded.is_empty() {
                                         view! {
                                             <div class="model-section">
                                                 <h2 class="model-section__title">"Loaded Models"</h2>
                                                 <div class="models-grid">
                                                     {loaded.into_iter().map(|m| {
                                                         let id_load = m.id.clone();
                                                         let id_unload = m.id.clone();
                                                         let id_edit = m.db_id
                                                             .map(|n| n.to_string())
                                                             .unwrap_or_else(|| m.id.clone());
                                                         let badge_class = model_status_badge_class(m.loaded);
                                                         let badge_label = model_status_badge_label(m.loaded);
                                                         let button_class = model_action_button_class(m.loaded);
                                                         let button_label = model_action_button_label(m.loaded);
                                                         view! {
                                                             <div class="model-card card">
                                                                 <div class="model-card__header">
                                                                     <span class="model-card__id">{model_display_name(&m)}</span>
                                                                     <span class={badge_class}>{badge_label}</span>
                                                                 </div>
                                                                 <div class="model-card__body">
                                                                     <div class="model-card__field">
                                                                         <span class="model-card__label">"Backend"</span>
                                                                         <span class="model-card__value text-mono">{m.backend}</span>
                                                                     </div>
                                                                 </div>
                                                                 <div class="model-card__actions">
                                                                     {if m.loaded {
                                                                         view! {
                                                                             <button
                                                                                 class={button_class}
                                                                                 prop:disabled=move || unload_pending.get()
                                                                                 on:click=move |_| { unload_action.dispatch(id_unload.clone()); }
                                                                             >
                                                                                 {button_label}
                                                                             </button>
                                                                         }.into_any()
                                                                     } else {
                                                                         view! {
                                                                             <button
                                                                                 class={button_class}
                                                                                 prop:disabled=move || load_pending.get()
                                                                                 on:click=move |_| { load_action.dispatch(id_load.clone()); }
                                                                             >
                                                                                 {button_label}
                                                                             </button>
                                                                         }.into_any()
                                                                     }}
                                                                     <A href=format!("/models/{}/edit", id_edit)>
                                                                         <button class="btn btn-secondary btn-sm">"Edit"</button>
                                                                     </A>
                                                                 </div>
                                                             </div>
                                                         }
                                                     }).collect::<Vec<_>>()}
                                                 </div>
                                             </div>
                                         }.into_any()

                                    } else {
                                        ().into_any()
                                    }}
                                    // Idle models section
                                    {if !idle.is_empty() {
                                         view! {
                                             <div class="model-section">
                                                 <h2 class="model-section__title">"Idle Models"</h2>
                                                 <div class="models-grid">
                                                     {idle.into_iter().map(|m| {
                                                         let id_load = m.id.clone();
                                                         let id_unload = m.id.clone();
                                                         let id_edit = m.db_id
                                                             .map(|n| n.to_string())
                                                             .unwrap_or_else(|| m.id.clone());
                                                         let badge_class = model_status_badge_class(m.loaded);
                                                         let badge_label = model_status_badge_label(m.loaded);
                                                         let button_class = model_action_button_class(m.loaded);
                                                         let button_label = model_action_button_label(m.loaded);
                                                         view! {
                                                             <div class="model-card card">
                                                                 <div class="model-card__header">
                                                                     <span class="model-card__id">{model_display_name(&m)}</span>
                                                                     <span class={badge_class}>{badge_label}</span>
                                                                 </div>
                                                                 <div class="model-card__body">
                                                                     <div class="model-card__field">
                                                                         <span class="model-card__label">"Backend"</span>
                                                                         <span class="model-card__value text-mono">{m.backend}</span>
                                                                     </div>
                                                                 </div>
                                                                 <div class="model-card__actions">
                                                                     {if m.loaded {
                                                                         view! {
                                                                             <button
                                                                                 class={button_class}
                                                                                 prop:disabled=move || unload_pending.get()
                                                                                 on:click=move |_| { unload_action.dispatch(id_unload.clone()); }
                                                                             >
                                                                                 {button_label}
                                                                             </button>
                                                                         }.into_any()
                                                                     } else {
                                                                         view! {
                                                                             <button
                                                                                 class={button_class}
                                                                                 prop:disabled=move || load_pending.get()
                                                                                 on:click=move |_| { load_action.dispatch(id_load.clone()); }
                                                                             >
                                                                                 {button_label}
                                                                             </button>
                                                                         }.into_any()
                                                                     }}
                                                                     <A href=format!("/models/{}/edit", id_edit)>
                                                                         <button class="btn btn-secondary btn-sm">"Edit"</button>
                                                                     </A>
                                                                 </div>
                                                             </div>
                                                         }
                                                     }).collect::<Vec<_>>()}
                                                 </div>
                                             </div>
                                         }.into_any()

                                    } else {
                                        ().into_any()
                                    }}
                                </div>
                            }.into_any()
                        }
                    }
                </section>
            }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MetricSample` must deserialize a payload that has no `models` field at
    /// all (older backend builds, cached responses) by defaulting to an empty
    /// `Vec`. The `#[serde(default)]` attribute on the field is what makes this
    /// work — without it, deserialization would fail with a `missing field`
    /// error and break the dashboard during a partial rollout.
    #[test]
    fn metric_sample_deserializes_without_models_field() {
        let json = r#"{
            "ts_unix_ms": 1700000000000,
            "cpu_usage_pct": 12.5,
            "ram_used_mib": 2048,
            "ram_total_mib": 16384,
            "gpu_utilization_pct": null,
            "vram": null,
            "models_loaded": 0
        }"#;

        let sample: MetricSample = serde_json::from_str(json)
            .expect("MetricSample without `models` must deserialize via #[serde(default)]");

        assert_eq!(sample.ts_unix_ms, 1_700_000_000_000);
        assert_eq!(sample.cpu_usage_pct, 12.5);
        assert_eq!(sample.ram_used_mib, 2048);
        assert_eq!(sample.ram_total_mib, 16_384);
        assert!(sample.gpu_utilization_pct.is_none());
        assert!(sample.vram.is_none());
        assert_eq!(sample.models_loaded, 0);
        assert!(
            sample.models.is_empty(),
            "missing `models` field must default to an empty Vec"
        );
    }

    /// `MetricsHistoryEntry` must correctly convert to `MetricSample`,
    /// mapping i64 fields to their corresponding types.
    #[test]
    fn metrics_history_entry_converts_to_metric_sample() {
        let entry = MetricsHistoryEntry {
            ts_unix_ms: 1_700_000_000_000,
            cpu_usage_pct: 45.5,
            ram_used_mib: 8192,
            ram_total_mib: 32768,
            gpu_utilization_pct: Some(85),
            vram_used_mib: Some(4096),
            vram_total_mib: Some(8192),
        };

        let sample: MetricSample = entry.into();

        assert_eq!(sample.ts_unix_ms, 1_700_000_000_000);
        assert!((sample.cpu_usage_pct - 45.5).abs() < f32::EPSILON);
        assert_eq!(sample.ram_used_mib, 8192);
        assert_eq!(sample.ram_total_mib, 32768);
        assert_eq!(sample.gpu_utilization_pct, Some(85));
        assert!(sample.vram.is_some());
        let vram = sample.vram.unwrap();
        assert_eq!(vram.used_mib, 4096);
        assert_eq!(vram.total_mib, 8192);
        assert_eq!(sample.models_loaded, 0);
        assert!(sample.models.is_empty());
    }

    /// `MetricsHistoryEntry` with null GPU/VRAM fields must produce a
    /// `MetricSample` with `None` for both.
    #[test]
    fn metrics_history_entry_converts_with_null_gpu() {
        let entry = MetricsHistoryEntry {
            ts_unix_ms: 1_700_000_000_000,
            cpu_usage_pct: 10.0,
            ram_used_mib: 2048,
            ram_total_mib: 16384,
            gpu_utilization_pct: None,
            vram_used_mib: None,
            vram_total_mib: None,
        };

        let sample: MetricSample = entry.into();

        assert!(sample.gpu_utilization_pct.is_none());
        assert!(sample.vram.is_none());
    }

    /// `MetricsHistoryEntry` with `vram_used_mib` present but
    /// `vram_total_mib` absent must produce `None` for `vram` (not a
    /// partial `VramInfo`).
    #[test]
    fn metrics_history_entry_partial_vram_produces_none() {
        let entry = MetricsHistoryEntry {
            ts_unix_ms: 1_700_000_000_000,
            cpu_usage_pct: 10.0,
            ram_used_mib: 2048,
            ram_total_mib: 16384,
            gpu_utilization_pct: Some(50),
            vram_used_mib: Some(4096),
            vram_total_mib: None,
        };

        let sample: MetricSample = entry.into();

        // vram should be None because total_mib is None
        assert!(sample.vram.is_none());
    }

    /// The `format_number` helper must produce comma-separated thousands.
    #[test]
    fn format_number_adds_commas() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(12345), "12,345");
        assert_eq!(format_number(123456), "123,456");
        assert_eq!(format_number(1234567), "1,234,567");
        assert_eq!(format_number(16384), "16,384");
        assert_eq!(format_number(65183), "65,183");
    }

    /// The dashboard's "Active Models" summary line shows how many of the
    /// configured models are currently loaded. The helper that backs that line
    /// must only count entries whose `loaded` flag is `true`, regardless of
    /// backend or id.
    #[test]
    fn loaded_model_count_only_counts_loaded_entries() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "llama_cpp".into(),
                loaded: true,
            },
            ModelStatus {
                id: "b".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "llama_cpp".into(),
                loaded: false,
            },
            ModelStatus {
                id: "c".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "ik_llama".into(),
                loaded: true,
            },
            ModelStatus {
                id: "d".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "ik_llama".into(),
                loaded: false,
            },
        ];

        assert_eq!(loaded_model_count(&models), 2);
    }

    /// With no models configured the helper returns `0`, which is what the
    /// empty-state UI renders alongside the "No models configured yet." copy.
    #[test]
    fn loaded_model_count_is_zero_for_empty_slice() {
        assert_eq!(loaded_model_count(&[]), 0);
    }

    /// Loaded models must use the success badge class so they visually pop
    /// against idle entries in the Active Models grid.
    #[test]
    fn model_status_badge_class_uses_success_when_loaded() {
        assert_eq!(model_status_badge_class(true), "badge badge-success");
    }

    /// Idle models must use the muted badge class so they recede compared to
    /// loaded entries — matching the convention used elsewhere on the
    /// `/models` page.
    #[test]
    fn model_status_badge_class_uses_muted_when_idle() {
        assert_eq!(model_status_badge_class(false), "badge badge-muted");
    }

    /// Badge text mirrors the badge colour: "Loaded" for loaded models,
    /// "Idle" for everything else. Tests both branches so a future renaming
    /// can't silently drift one of them.
    #[test]
    fn model_status_badge_label_distinguishes_loaded_and_idle() {
        assert_eq!(model_status_badge_label(true), "Loaded");
        assert_eq!(model_status_badge_label(false), "Idle");
    }

    /// Loaded models surface an Unload action — destructive styling so the
    /// user understands clicking it tears down a running server.
    #[test]
    fn model_action_button_class_uses_danger_when_loaded() {
        assert_eq!(model_action_button_class(true), "btn btn-danger btn-sm");
    }

    /// Idle models surface a Load action — affirmative styling so the user
    /// understands clicking it spins up a server.
    #[test]
    fn model_action_button_class_uses_success_when_idle() {
        assert_eq!(model_action_button_class(false), "btn btn-success btn-sm");
    }

    /// Action button labels must match their visual styling: "Unload" for
    /// loaded models, "Load" for idle ones. Tests both branches so the
    /// label and class helpers stay in lockstep.
    #[test]
    fn model_action_button_label_distinguishes_loaded_and_idle() {
        assert_eq!(model_action_button_label(true), "Unload");
        assert_eq!(model_action_button_label(false), "Load");
    }

    /// When the backend includes a populated `models` array, every `ModelStatus`
    /// must round-trip with its `id`, `backend`, and `loaded` fields preserved.
    /// This is the wire format the dashboard's UI rendering depends on.
    #[test]
    fn metric_sample_deserializes_models_field() {
        let json = r#"{
            "ts_unix_ms": 1700000000000,
            "cpu_usage_pct": 0.0,
            "ram_used_mib": 0,
            "ram_total_mib": 0,
            "gpu_utilization_pct": null,
            "vram": null,
            "models_loaded": 1,
            "models": [
                { "id": "alpha", "api_name": "org/alpha", "backend": "llama_cpp", "loaded": true },
                { "id": "beta",  "api_name": "org/beta",  "backend": "ik_llama",  "loaded": false }
            ]
        }"#;

        let sample: MetricSample =
            serde_json::from_str(json).expect("MetricSample with `models` must deserialize");

        assert_eq!(sample.models.len(), 2);

        assert_eq!(sample.models[0].id, "alpha");
        assert_eq!(sample.models[0].api_name, Some("org/alpha".to_string()));
        assert_eq!(sample.models[0].backend, "llama_cpp");
        assert!(sample.models[0].loaded);

        assert_eq!(sample.models[1].id, "beta");
        assert_eq!(sample.models[1].api_name, Some("org/beta".to_string()));
        assert_eq!(sample.models[1].backend, "ik_llama");
        assert!(!sample.models[1].loaded);
    }
}
