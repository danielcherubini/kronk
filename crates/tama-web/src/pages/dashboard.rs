use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;



use crate::components::sparkline::SparklineChart;
use crate::utils::{extract_and_store_csrf_token, post_request};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetricSample {
    ts_unix_ms: i64,
    cpu_usage_pct: f32,
    ram_used_mib: u64,
    ram_total_mib: u64,
    gpu_utilization_pct: Option<u8>,
    vram: Option<VramInfo>,
    models_loaded: u64,
    /// Per-model loaded/idle status mirrored from `tama_core::gpu::MetricSample.models`.
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

/// Frontend mirror of `tama_core::gpu::ModelStatus`.
///
/// Kept private to this module so the dashboard owns its wire shape; the only
/// contract with the backend is the JSON field names, which must match the
/// server-side struct exactly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(deprecated)]
struct ModelStatus {
    id: String,
    #[serde(default)]
    db_id: Option<i64>,
    #[serde(default)]
    api_name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    backend: String,
    #[deprecated(since = "1.45.0", note = "use state field instead")]
    #[serde(default)]
    loaded: bool,
    /// Lifecycle state: idle, loading, ready, unloading, failed.
    #[serde(default)]
    state: String,
    #[serde(default)]
    quant: Option<String>,
    #[serde(default)]
    context_length: Option<u32>,
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
    models.iter().filter(|m| m.state == "ready").count()
}

/// CSS class string used for the per-model status badge in the
/// "Active Models" grid. Maps lifecycle states to colour classes.
fn model_status_badge_class(state: &str) -> &'static str {
    match state {
        "ready" => "badge badge-success",
        "loading" => "badge badge-info",
        "unloading" => "badge badge-warning",
        "failed" => "badge badge-error",
        _ => "badge badge-muted",
    }
}

/// Human-readable label that pairs with [`model_status_badge_class`].
fn model_status_badge_label(state: &str) -> &'static str {
    match state {
        "ready" => "Loaded",
        "loading" => "Loading",
        "unloading" => "Unloading",
        "failed" => "Failed",
        _ => "Idle",
    }
}

/// CSS class string for the load/unload action button in a model card.
/// Ready models render an "Unload" button (btn-danger),
/// loading/unloading/failed show muted buttons,
/// idle shows a "Load" button (btn-success).
fn model_action_button_class(state: &str) -> &'static str {
    match state {
        "ready" => "btn btn-danger btn-sm",
        "loading" => "btn btn-secondary btn-sm",
        "unloading" => "btn btn-secondary btn-sm",
        "failed" => "btn btn-warning btn-sm",
        _ => "btn btn-success btn-sm",
    }
}

/// Human-readable label that pairs with [`model_action_button_class`].
fn model_action_button_label(state: &str) -> &'static str {
    match state {
        "ready" => "Unload",
        "loading" => "Loading…",
        "unloading" => "Unloading…",
        "failed" => "Retry",
        _ => "Load",
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

/// Renders a single model row. Isolated component so only changed rows rebuild
/// when metrics update — prevents the entire list from being destroyed/recreated.
#[component]
fn ModelRow(
    id: String,
    db_id: Option<i64>,
    display_name: String,
    quant_display: String,
    context_display: String,
    backend_name: String,
    state: String,
    load_pending: RwSignal<bool>,
    unload_pending: RwSignal<bool>,
    on_load: Callback<String>,
    on_unload: Callback<String>,
) -> impl IntoView {
    let badge_class = model_status_badge_class(&state);
    let badge_label = model_status_badge_label(&state);
    let button_class = model_action_button_class(&state);
    let button_label = model_action_button_label(&state);
    // Clone values needed in closures before they're consumed by the view!
    let id_for_load = id.clone();
    let id_for_edit = id.clone();
    let id_for_unload = id.clone();
    let backend_for_logs = backend_name.clone();

    view! {
        <div class="model-row card">
            <span class="model-row__name">{display_name}</span>
            <span class="model-row__meta">{quant_display}</span>
            <span class="model-row__meta">{context_display}</span>
            <span class="model-row__backend text-mono">{backend_name}</span>
            <div class="model-row__actions">
                <span class={badge_class}>{badge_label}</span>
                {if matches!(state.as_str(), "ready") {
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=move || unload_pending.get()
                            on:click=move |_| { on_unload.run(id_for_unload.clone()); }
                        >
                            {button_label}
                        </button>
                    }.into_any()
                } else if matches!(state.as_str(), "loading" | "unloading") {
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=true
                        >
                            {button_label}
                        </button>
                    }.into_any()
                } else {
                    // idle, failed → Load or Retry
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=move || load_pending.get()
                            on:click=move |_| { on_load.run(id_for_load.clone()); }
                        >
                            {button_label}
                        </button>
                    }.into_any()
                }}
                <A
                    href=format!("/logs?backend={}", backend_for_logs)
                    attr:class="btn btn-secondary btn-sm"
                    attr:title="View backend logs"
                >
                    "Logs"
                </A>
                <A
                    href=format!("/models/{}/edit", db_id.map(|n| n.to_string()).unwrap_or_else(|| id_for_edit.clone()))
                    attr:class="btn btn-secondary btn-sm"
                >
                    "Edit"
                </A>
            </div>
        </div>
    }
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
                gloo_net::http::Request::get("/tama/v1/system/metrics/history?limit=450")
                    .send()
                    .await
            {
                extract_and_store_csrf_token(&resp);
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

        let es = match web_sys::EventSource::new("/tama/v1/system/metrics/stream") {
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
                            // Sort by id (stable order, matching the backend)
                            let mut sorted = models;
                            sorted.sort_by(|a, b| a.id.cmp(&b.id));
                            view! {
                                <div class="models-list">
                                    {sorted.into_iter().map(|m| {
                                        let display_name = model_display_name(&m);
                                        let quant_display: String = m
                                            .quant
                                            .as_deref()
                                            .unwrap_or("\u{2014}")
                                            .into();
                                        let context_display = m.context_length.map(|n| {
                                            if n >= 1024 && n % 1024 == 0 {
                                                format!("{}k", n / 1024)
                                            } else if n >= 1000 && n % 1000 == 0 {
                                                format!("{}k", n / 1000)
                                            } else {
                                                n.to_string()
                                            }
                                        }).unwrap_or_else(|| "—".to_string());
                                        let backend_name = format!("{}_{}", m.backend, m.id);
                                        let id = m.id.clone();
                                        let db_id = m.db_id;
                                        let state = m.state.clone();
                                        let on_load_cb = Callback::new(move |id: String| {
                                            load_action.dispatch(id);
                                        });
                                        let on_unload_cb = Callback::new(move |id: String| {
                                            unload_action.dispatch(id);
                                        });
                                        view! {
                                            <ModelRow
                                                id=id
                                                db_id=db_id
                                                display_name=display_name
                                                quant_display=quant_display
                                                context_display=context_display
                                                backend_name=backend_name
                                                state=state
                                                load_pending=load_busy
                                                unload_pending=unload_busy
                                                on_load=on_load_cb
                                                on_unload=on_unload_cb
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
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "llama_cpp".into(),
                state: "idle".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "c".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "ik_llama".into(),
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "d".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "ik_llama".into(),
                state: "idle".into(),
                ..Default::default()
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

    /// Loaded (ready) models must use the success badge class so they visually pop
    /// against idle entries in the Active Models grid.
    #[test]
    fn model_status_badge_class_uses_success_when_ready() {
        assert_eq!(model_status_badge_class("ready"), "badge badge-success");
    }

    /// Idle models must use the muted badge class so they recede compared to
    /// loaded entries — matching the convention used elsewhere on the
    /// `/models` page.
    #[test]
    fn model_status_badge_class_uses_muted_when_idle() {
        assert_eq!(model_status_badge_class("idle"), "badge badge-muted");
    }

    /// Badge text mirrors the badge colour: "Loaded" for ready models,
    /// "Idle" for everything else. Tests both branches so a future renaming
    /// can't silently drift one of them.
    #[test]
    fn model_status_badge_label_distinguishes_ready_and_idle() {
        assert_eq!(model_status_badge_label("ready"), "Loaded");
        assert_eq!(model_status_badge_label("idle"), "Idle");
    }

    /// Ready models surface an Unload action — destructive styling so the
    /// user understands clicking it tears down a running server.
    #[test]
    fn model_action_button_class_uses_danger_when_ready() {
        assert_eq!(model_action_button_class("ready"), "btn btn-danger btn-sm");
    }

    /// Idle models surface a Load action — affirmative styling so the user
    /// understands clicking it spins up a server.
    #[test]
    fn model_action_button_class_uses_success_when_idle() {
        assert_eq!(model_action_button_class("idle"), "btn btn-success btn-sm");
    }

    /// Loading models get a disabled secondary button.
    #[test]
    fn model_action_button_class_uses_secondary_when_loading() {
        assert_eq!(
            model_action_button_class("loading"),
            "btn btn-secondary btn-sm"
        );
    }

    /// Unloading models get a disabled secondary button.
    #[test]
    fn model_action_button_class_uses_secondary_when_unloading() {
        assert_eq!(
            model_action_button_class("unloading"),
            "btn btn-secondary btn-sm"
        );
    }

    /// Failed models get a warning button (Retry).
    #[test]
    fn model_action_button_class_uses_warning_when_failed() {
        assert_eq!(
            model_action_button_class("failed"),
            "btn btn-warning btn-sm"
        );
    }

    /// Action button labels must match their visual styling.
    #[test]
    fn model_action_button_label_distinguishes_states() {
        assert_eq!(model_action_button_label("ready"), "Unload");
        assert_eq!(model_action_button_label("idle"), "Load");
        assert_eq!(model_action_button_label("loading"), "Loading…");
        assert_eq!(model_action_button_label("unloading"), "Unloading…");
        assert_eq!(model_action_button_label("failed"), "Retry");
    }

    /// Status badge class and label helpers map states correctly.
    #[test]
    fn model_status_badge_class_and_label_map_all_states() {
        assert_eq!(model_status_badge_class("ready"), "badge badge-success");
        assert_eq!(model_status_badge_label("ready"), "Loaded");

        assert_eq!(model_status_badge_class("loading"), "badge badge-info");
        assert_eq!(model_status_badge_label("loading"), "Loading");

        assert_eq!(model_status_badge_class("unloading"), "badge badge-warning");
        assert_eq!(model_status_badge_label("unloading"), "Unloading");

        assert_eq!(model_status_badge_class("failed"), "badge badge-error");
        assert_eq!(model_status_badge_label("failed"), "Failed");

        assert_eq!(model_status_badge_class("idle"), "badge badge-muted");
        assert_eq!(model_status_badge_label("idle"), "Idle");
    }

    /// When the backend includes a populated `models` array, every `ModelStatus`
    /// must round-trip with its `id`, `backend`, and `state` fields preserved.
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
                { "id": "alpha", "api_name": "org/alpha", "backend": "llama_cpp", "loaded": true, "state": "ready" },
                { "id": "beta",  "api_name": "org/beta",  "backend": "ik_llama",  "loaded": false, "state": "idle" }
            ]
        }"#;

        let sample: MetricSample =
            serde_json::from_str(json).expect("MetricSample with `models` must deserialize");

        assert_eq!(sample.models.len(), 2);

        assert_eq!(sample.models[0].id, "alpha");
        assert_eq!(sample.models[0].api_name, Some("org/alpha".to_string()));
        assert_eq!(sample.models[0].backend, "llama_cpp");
        assert_eq!(sample.models[0].state, "ready");

        assert_eq!(sample.models[1].id, "beta");
        assert_eq!(sample.models[1].api_name, Some("org/beta".to_string()));
        assert_eq!(sample.models[1].backend, "ik_llama");
        assert_eq!(sample.models[1].state, "idle");
    }
}
