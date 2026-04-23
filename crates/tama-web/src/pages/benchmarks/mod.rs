//! Benchmarks page — run llama-bench and spec-decoding benchmarks from the web UI.

mod spec_bench;
mod types;
mod utils;

use std::collections::{BTreeMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use self::spec_bench::SpecBench;
use self::types::{BenchmarkPreset, HistoryEntry};
use self::utils::{format_relative, format_timestamp};
use crate::components::job_log_panel::JobLogPanel;
use crate::utils::{extract_and_store_csrf_token, post_request};

/// Render "mean ± stddev" with one decimal place, or a single value when
/// stddev rounds to zero.
fn format_mean_stddev(mean: f64, stddev: f64) -> String {
    if stddev > 0.05 {
        format!("{:.1} ± {:.1}", mean, stddev)
    } else {
        format!("{:.1}", mean)
    }
}

/// Render a table of per-summary results, adding columns for whichever
/// per-run knobs actually vary between rows. A column for a constant knob is
/// redundant with the header card and would just add noise — so we only add
/// one when the field has more than one distinct value across the rows.
///
/// Shared between the live benchmark results and the history accordion detail
/// panel so both look identical.
fn render_summaries_table(summaries: &[serde_json::Value]) -> impl IntoView {
    let get_u64 = |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_u64());
    let get_str =
        |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_str()).map(|x| x.to_string());
    let get_bool = |s: &serde_json::Value, k: &str| s.get(k).and_then(|v| v.as_bool());

    // Which per-run knobs vary across rows? Only those get a column.
    let distinct_u64 =
        |k: &str| -> HashSet<u64> { summaries.iter().filter_map(|s| get_u64(s, k)).collect() };
    let distinct_str =
        |k: &str| -> HashSet<String> { summaries.iter().filter_map(|s| get_str(s, k)).collect() };
    let distinct_bool =
        |k: &str| -> HashSet<bool> { summaries.iter().filter_map(|s| get_bool(s, k)).collect() };

    let show_depth = distinct_u64("n_depth").len() > 1;
    let show_batch = distinct_u64("n_batch").len() > 1;
    let show_ubatch = distinct_u64("n_ubatch").len() > 1;
    // KV cache is expressed by two fields. Treat them as a single "KV" column
    // that varies when either side varies.
    let show_kv = distinct_str("type_k").len() > 1 || distinct_str("type_v").len() > 1;
    let show_fa = distinct_bool("flash_attn").len() > 1;

    let rows: Vec<_> = summaries
        .iter()
        .map(|s| {
            let n_prompt = get_u64(s, "prompt_tokens").unwrap_or(0);
            let n_gen = get_u64(s, "gen_tokens").unwrap_or(0);
            let pp_mean = s.get("pp_mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let pp_stddev = s.get("pp_stddev").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tg_mean = s.get("tg_mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tg_stddev = s.get("tg_stddev").and_then(|v| v.as_f64()).unwrap_or(0.0);

            let (test_label, phase, value) = if n_prompt > 0 && n_gen == 0 {
                (
                    format!("pp{}", n_prompt),
                    "PP".to_string(),
                    format_mean_stddev(pp_mean, pp_stddev),
                )
            } else if n_prompt == 0 && n_gen > 0 {
                (
                    format!("tg{}", n_gen),
                    "TG".to_string(),
                    format_mean_stddev(tg_mean, tg_stddev),
                )
            } else {
                (
                    format!("pp{}+tg{}", n_prompt, n_gen),
                    "TG".to_string(),
                    format_mean_stddev(tg_mean, tg_stddev),
                )
            };

            let depth = get_u64(s, "n_depth")
                .map(|v| v.to_string())
                .unwrap_or_default();
            let batch = get_u64(s, "n_batch")
                .map(|v| v.to_string())
                .unwrap_or_default();
            let ubatch = get_u64(s, "n_ubatch")
                .map(|v| v.to_string())
                .unwrap_or_default();
            let kv = {
                let k = get_str(s, "type_k").unwrap_or_default();
                let v = get_str(s, "type_v").unwrap_or_default();
                if k.is_empty() && v.is_empty() {
                    String::new()
                } else if k == v {
                    k
                } else {
                    format!("{}/{}", k, v)
                }
            };
            let fa = get_bool(s, "flash_attn")
                .map(|v| if v { "on" } else { "off" }.to_string())
                .unwrap_or_default();

            (test_label, phase, value, depth, batch, ubatch, kv, fa)
        })
        .collect();

    view! {
        <table class="table table-striped">
            <thead>
                <tr>
                    <th>"Test"</th>
                    <th>"Phase"</th>
                    {show_depth.then(|| view! { <th>"Depth"</th> })}
                    {show_batch.then(|| view! { <th>"Batch"</th> })}
                    {show_ubatch.then(|| view! { <th>"µ-batch"</th> })}
                    {show_kv.then(|| view! { <th>"KV"</th> })}
                    {show_fa.then(|| view! { <th>"Flash"</th> })}
                    <th class="text-right">"t/s (± stddev)"</th>
                </tr>
            </thead>
            <tbody>
                {rows.into_iter().map(|(test_label, phase, value, depth, batch, ubatch, kv, fa)| {
                    view! {
                        <tr>
                            <td class="text-mono">{test_label}</td>
                            <td>{phase}</td>
                            {show_depth.then(|| view! { <td class="text-mono">{depth}</td> })}
                            {show_batch.then(|| view! { <td class="text-mono">{batch}</td> })}
                            {show_ubatch.then(|| view! { <td class="text-mono">{ubatch}</td> })}
                            {show_kv.then(|| view! { <td class="text-mono">{kv}</td> })}
                            {show_fa.then(|| view! { <td>{fa}</td> })}
                            <td class="text-mono text-right">{value}</td>
                        </tr>
                    }
                }).collect::<Vec<_>>()}
            </tbody>
        </table>
    }
}

#[component]
pub fn Benchmarks() -> impl IntoView {
    // Model selection — two steps: pick display_name, then pick a quant. The
    // resolved id (db_id as a string) is what we actually submit.
    let selected_display_name = RwSignal::new(String::new());
    let selected_model = RwSignal::new(String::new());
    // Raw (id, display_name, quant) entries from /tama/v1/models.
    let available_models = RwSignal::new(Vec::<(String, String, String)>::new());

    // Backend selection — which backend's llama-bench to use
    let selected_backend = RwSignal::new(String::new());
    let available_backends = RwSignal::new(Vec::<(String, String)>::new()); // (name, display_name)

    // Test configuration
    let pp_sizes_str = RwSignal::new("512".to_string());
    let tg_sizes_str = RwSignal::new("128".to_string());
    let runs = RwSignal::new(3u32);
    let warmup = RwSignal::new(1u32);
    let threads_str = RwSignal::new("auto".to_string());
    let ngl_range = RwSignal::new("".to_string());
    let ctx_override = RwSignal::new("".to_string());

    // Methodology-driven knobs (from llm-inference-tuning-methodology.md):
    //   -b / -ub  : batch / micro-batch — biggest single PP win documented (~36%)
    //   -ctk/-ctv : KV cache quant — MUST be matched or attention falls back to CPU
    //   -d        : depth — pre-fill N tokens, essential when evaluating KV quant
    //   -fa       : flash attention — default on for modern backends
    let batch_sizes_str = RwSignal::new("".to_string());
    let ubatch_sizes_str = RwSignal::new("".to_string());
    let kv_cache_type = RwSignal::new("default".to_string());
    let depth_str = RwSignal::new("".to_string());
    let flash_attn = RwSignal::new(true);

    // Job state — is_running tracks whether a benchmark is currently running
    let is_running = RwSignal::new(false);
    let current_job_id = RwSignal::new(Option::<String>::None);
    // Full BenchReport payload delivered over the SSE "result" event, parsed
    // once for display. The frontend expects `{ model_info, config, summaries,
    // load_time_ms, vram }`.
    let benchmark_results = RwSignal::new(Option::<serde_json::Value>::None);

    // History state — always visible.
    let history = RwSignal::new(Vec::<HistoryEntry>::new());
    // IDs of history rows whose per-summary detail panel is open. Each row acts
    // as an accordion toggle — clicking flips its id in this set.
    let expanded_history = RwSignal::new(HashSet::<i64>::new());

    // Refresh trigger — increment to force a refetch
    let model_refresh = RwSignal::new(0u32);

    // Fetch available models on mount.
    Effect::new(move |_| {
        let _ = model_refresh.get();
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/tama/v1/models").send().await {
                extract_and_store_csrf_token(&resp);
                if let Ok(root) = resp.json::<serde_json::Value>().await {
                    if let Some(models_arr) = root.get("models").and_then(|v| v.as_array()) {
                        let model_list: Vec<(String, String, String)> =
                            models_arr.iter().filter_map(types::parse_model).collect();
                        available_models.update(|list| *list = model_list);
                    }
                }
            }
        });
    });

    // Fetch available backends for llama-bench selection.
    {
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/tama/v1/backends")
                .send()
                .await
            {
                extract_and_store_csrf_token(&resp);
                if let Ok(root) = resp.json::<serde_json::Value>().await {
                    if let Some(backends_arr) = root.get("backends").and_then(|v| v.as_array()) {
                        let backend_list: Vec<(String, String)> = backends_arr
                            .iter()
                            .filter_map(|b| {
                                let name = b.get("name")?.as_str()?.to_string();
                                let display = b
                                    .get("display_name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&name)
                                    .to_string();
                                Some((name, display))
                            })
                            .collect();
                        available_backends.update(|list| *list = backend_list);
                    }
                }
            }
        });
    }

    // Trigger for history refetch — incremented whenever we want to reload.
    let history_refresh = RwSignal::new(0u32);

    // Fetch benchmark history on mount and whenever history_refresh changes.
    Effect::new(move |_| {
        let _ = history_refresh.get();
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/tama/v1/benchmarks/history")
                .send()
                .await
            {
                extract_and_store_csrf_token(&resp);
                if let Ok(entries) = resp.json::<Vec<HistoryEntry>>().await {
                    history.set(entries);
                }
            }
        });
    });

    // Parse helpers. Zero is a meaningful value — `-p 0` pins llama-bench to
    // pure-TG mode, which is exactly what the KV-quant and depth-validation
    // phases of the methodology want. So we only drop empty/unparsable
    // tokens, not zeros.
    let parse_sizes = move |s: &str| -> Vec<u32> {
        s.split(',')
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .filter_map(|v| v.parse::<u32>().ok())
            .collect()
    };

    let parse_threads = move |s: &str| -> Option<Vec<u32>> {
        if s.trim().to_lowercase() == "auto" || s.trim().is_empty() {
            None
        } else {
            Some(
                s.split(',')
                    .map(|v| v.trim().parse::<u32>().unwrap_or(0))
                    .filter(|v| *v > 0)
                    .collect(),
            )
        }
    };

    // Apply preset handler — writes every methodology knob (not just the
    // core sizes) so loading a preset fully reproduces the phase it maps to.
    let apply_preset_handler = move |preset: BenchmarkPreset| {
        let join_u32 = |xs: &[u32]| {
            xs.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };

        pp_sizes_str.set(join_u32(preset.pp_sizes));
        tg_sizes_str.set(join_u32(preset.tg_sizes));
        runs.set(preset.runs);
        threads_str.set(
            preset
                .threads
                .as_ref()
                .map(|t| join_u32(t))
                .unwrap_or_else(|| "auto".to_string()),
        );
        ngl_range.set(preset.ngl_range.unwrap_or("").to_string());

        batch_sizes_str.set(join_u32(preset.batch_sizes));
        ubatch_sizes_str.set(join_u32(preset.ubatch_sizes));
        kv_cache_type.set(preset.kv_cache_type.unwrap_or("default").to_string());
        depth_str.set(join_u32(preset.depth));
        if let Some(fa) = preset.flash_attn {
            flash_attn.set(fa);
        }
    };

    // Submit benchmark and connect SSE
    let submit_benchmark = move || {
        let model_id = selected_model.get();
        let pp = parse_sizes(&pp_sizes_str.get());
        let tg = parse_sizes(&tg_sizes_str.get());
        let runs_val = runs.get();
        let warmup_val = warmup.get();
        let threads = parse_threads(&threads_str.get());
        let ngl = if ngl_range.get().is_empty() {
            None
        } else {
            Some(ngl_range.get())
        };
        let ctx = if ctx_override.get().is_empty() {
            None
        } else {
            ctx_override.get().parse::<u32>().ok()
        };

        // Methodology knobs
        let batch_sizes = parse_sizes(&batch_sizes_str.get());
        let ubatch_sizes = parse_sizes(&ubatch_sizes_str.get());
        let kv = kv_cache_type.get();
        let kv_payload: Option<String> = if kv == "default" { None } else { Some(kv) };
        let depth = parse_sizes(&depth_str.get());
        let fa_payload: Option<bool> = Some(flash_attn.get());

        // Clear any previous results and mark the job as running.
        benchmark_results.set(None);
        is_running.set(true);
        current_job_id.set(None);

        spawn_local(async move {
            let backend_name = if selected_backend.get().is_empty() {
                None
            } else {
                Some(selected_backend.get())
            };
            let body = serde_json::json!({
                "model_id": model_id,
                "backend_name": backend_name,
                "pp_sizes": pp,
                "tg_sizes": tg,
                "runs": runs_val,
                "warmup": warmup_val,
                "threads": threads,
                "ngl_range": ngl,
                "ctx_override": ctx,
                "batch_sizes": batch_sizes,
                "ubatch_sizes": ubatch_sizes,
                "kv_cache_type": kv_payload,
                "depth": depth,
                "flash_attn": fa_payload,
            });

            let submitted = async {
                let resp = post_request("/tama/v1/benchmarks/run")
                    .header("Content-Type", "application/json")
                    .body(body.to_string())
                    .ok()?;
                let resp = resp.send().await.ok()?;
                let body = resp.json::<serde_json::Value>().await.ok()?;
                body.get("job_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }
            .await;

            match submitted {
                Some(job_id) => {
                    current_job_id.set(Some(job_id));
                }
                None => {
                    // Submission failed — roll back is_running so the user can retry.
                    is_running.set(false);
                }
            }
        });
    };

    // Callbacks passed to JobLogPanel.
    let on_result_cb = Callback::new(move |results_json: String| {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&results_json) {
            benchmark_results.set(Some(parsed));
        }
        history_refresh.update(|n| *n += 1);
    });
    let on_status_cb = Callback::new(move |status: String| {
        if status != "running" {
            is_running.set(false);
            history_refresh.update(|n| *n += 1);
        }
    });

    // When the display_name changes, auto-select the first quant so the id is
    // always populated.
    Effect::new(move |_| {
        let dn = selected_display_name.get();
        let models = available_models.get();
        if let Some((id, _, _)) = models.iter().find(|(_, name, _)| name == &dn) {
            selected_model.set(id.clone());
        } else {
            selected_model.set(String::new());
        }
    });

    // Read-only splits for views
    let (available_models_sig, _) = available_models.split();
    let (selected_display_sig, _) = selected_display_name.split();
    let (selected_model_sig, _) = selected_model.split();
    let (available_backends_sig, _) = available_backends.split();
    let (pp_sizes_sig, _) = pp_sizes_str.split();
    let (tg_sizes_sig, _) = tg_sizes_str.split();
    let (runs_sig, _) = runs.split();
    let (warmup_sig, _) = warmup.split();
    let (threads_sig, _) = threads_str.split();
    let (ngl_sig, _) = ngl_range.split();
    let (batch_sig, _) = batch_sizes_str.split();
    let (ubatch_sig, _) = ubatch_sizes_str.split();
    let (kv_sig, _) = kv_cache_type.split();
    let (depth_sig, _) = depth_str.split();
    let (fa_sig, _) = flash_attn.split();
    let (history_sig, _) = history.split();
    let (expanded_sig, _) = expanded_history.split();
    let (is_running_sig, _) = is_running.split();
    let (current_job_id_sig, _) = current_job_id.split();

    // Tab toggle — switch between llama-bench and spec-decoding views.
    let active_tab = RwSignal::new("llama-bench");

    view! {
        <div class="page-header">
            <h1>"Benchmarks"</h1>
        </div>

        // Tab buttons
        <div class="tab-buttons">
            <button class=move || if active_tab.get() == "llama-bench" { "btn btn-sm btn-primary" } else { "btn btn-sm btn-outline-secondary" }
                    on:click=move |_| active_tab.set("llama-bench")>
                "LLaMA-Bench"
            </button>
            <button class=move || if active_tab.get() == "spec-decode" { "btn btn-sm btn-primary" } else { "btn btn-sm btn-outline-secondary" }
                    on:click=move |_| active_tab.set("spec-decode")>
                "Spec Decoding"
            </button>
        </div>

        // LLaMA-Bench tab content
        {move || {
            if active_tab.get() == "spec-decode" {
                view! { <SpecBench /> }.into_any()
            } else {
                view! {
                    // Model selection — two-step: model, then quant. Models can ship with
        // multiple quants (e.g. Q4_K_M vs Q6_K) and the delta matters for
        // benchmarking, so we make the quant an explicit choice.
        <section class="card">
            <h3>"Model"</h3>
            <div class="grid-2">
                <div class="form-group">
                    <label>"Model"</label>
                    <select
                        class="form-select"
                        on:change=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                            selected_display_name.set(val);
                        }
                    >
                        <option value="" disabled selected=move || selected_display_sig.get().is_empty()>"Select a model..."</option>
                        {move || {
                            let models = available_models_sig.get();
                            // Deduplicate by display_name; BTreeMap keeps them
                            // sorted alphabetically for stable rendering.
                            let mut grouped: BTreeMap<String, ()> = BTreeMap::new();
                            for (_, name, _) in models.iter() {
                                grouped.insert(name.clone(), ());
                            }
                            grouped.keys().map(|name| {
                                let value = name.clone();
                                let label = name.clone();
                                view! {
                                    <option value=value>{label}</option>
                                }.into_any()
                            }).collect::<Vec<_>>()
                        }}
                    </select>
                </div>
                <div class="form-group">
                    <label>"Quant"</label>
                    <select
                        class="form-select"
                        prop:disabled=move || selected_display_sig.get().is_empty()
                        on:change=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                            selected_model.set(val);
                        }
                    >
                        <option value="" disabled>"Select quant..."</option>
                        {move || {
                            let models = available_models_sig.get();
                            let dn = selected_display_sig.get();
                            let selected_id = selected_model_sig.get();
                            models.iter()
                                .filter(|(_, name, _)| name == &dn)
                                .map(|(id, _, quant)| {
                                    let id_clone = id.clone();
                                    let is_selected = id == &selected_id;
                                    let label = if quant.is_empty() { "—".to_string() } else { quant.clone() };
                                    view! {
                                        <option value=id_clone selected=is_selected>{label}</option>
                                    }.into_any()
                                }).collect::<Vec<_>>()
                        }}
                    </select>
                </div>
            </div>
        </section>

        // Backend selection (which llama-bench to use)
        <section class="card">
            <h3>"Backend"</h3>
            <select
                class="form-select"
                on:change=move |e| {
                    let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                    selected_backend.set(val);
                }
            >
                <option value="">"Auto (model's backend)"</option>
                {move || {
                    let backends = available_backends_sig.get();
                    backends.iter().map(|(name, display)| {
                        let name_clone = name.clone();
                        let display_clone = display.clone();
                        view! {
                            <option value=name_clone>{display_clone}</option>
                        }.into_any()
                    }).collect::<Vec<_>>()
                }}
            </select>
            <small class="bench-hint">
                "Select a specific backend's llama-bench, or leave empty to use the model's backend."
            </small>
        </section>

        // Test configuration
        <section class="card">
            <h3>"Test Configuration"</h3>
            <div class="grid-2">
                <div class="form-group">
                    <label>"Prompt sizes (tokens)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || pp_sizes_sig.get()
                        on:input=move |e| { pp_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 128,256,512"</small>
                </div>
                <div class="form-group">
                    <label>"Generation lengths (tokens)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || tg_sizes_sig.get()
                        on:input=move |e| { tg_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 32,64,128"</small>
                </div>
                <div class="form-group">
                    <label>"Runs"</label>
                    <input
                        type="number"
                        class="form-control"
                        prop:value=move || runs_sig.get()
                        min="1" max="20"
                        on:input=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value();
                            if let Ok(n) = val.parse::<u32>() { runs.set(n); }
                        }
                    />
                </div>
                <div class="form-group">
                    <label>"Warmup runs"</label>
                    <input
                        type="number"
                        class="form-control"
                        prop:value=move || warmup_sig.get()
                        min="0" max="10"
                        on:input=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value();
                            if let Ok(n) = val.parse::<u32>() { warmup.set(n); }
                        }
                    />
                </div>
                <div class="form-group">
                    <label>"Threads"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || threads_sig.get()
                        on:input=move |e| { threads_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"auto, or comma-separated e.g. 4,8,16"</small>
                </div>
                <div class="form-group">
                    <label>"GPU layers range (sweet spot)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || ngl_sig.get()
                        on:input=move |e| { ngl_range.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"e.g. 0-99+1 to sweep, or empty for all"</small>
                </div>
            </div>
        </section>

        // Advanced tuning — knobs from the LLM-inference-tuning methodology.
        // Each one is worth a full paragraph of explanation; the small text
        // below each field is the cheat-sheet version.
        <section class="card">
            <h3>"Advanced Tuning"</h3>
            <div class="grid-2">
                <div class="form-group">
                    <label>"Batch size (-b)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || batch_sig.get()
                        on:input=move |e| { batch_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Logical batch. Try 512,1024,2048 — can yield up to ~36% PP."</small>
                </div>
                <div class="form-group">
                    <label>"Micro-batch size (-ub)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || ubatch_sig.get()
                        on:input=move |e| { ubatch_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Physical micro-batch. Typically ≤ batch size."</small>
                </div>
                <div class="form-group">
                    <label>"KV cache type (-ctk/-ctv)"</label>
                    <select
                        class="form-select"
                        on:change=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                            kv_cache_type.set(val);
                        }
                    >
                        {move || {
                            let current = kv_sig.get();
                            vec!["default", "f16", "q8_0", "q4_0"].into_iter().map(|opt| {
                                let opt_str = opt.to_string();
                                let selected = opt == current;
                                let label = match opt {
                                    "default" => "Default (backend)",
                                    other => other,
                                };
                                view! {
                                    <option value=opt_str selected=selected>{label}</option>
                                }.into_any()
                            }).collect::<Vec<_>>()
                        }}
                    </select>
                    <small class="text-muted">"Applied to both K and V. Mismatched pair = CPU attention fallback."</small>
                </div>
                <div class="form-group">
                    <label>"Depth (-d)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || depth_sig.get()
                        on:input=move |e| { depth_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Pre-fill tokens before timing. e.g. 0,4096,16384 when testing KV quant."</small>
                </div>
                <div class="form-group">
                    <div class="form-check">
                        <input
                            id="bench-flash-attn"
                            type="checkbox"
                            prop:checked=move || fa_sig.get()
                            on:change=move |e| {
                                let checked = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().checked();
                                flash_attn.set(checked);
                            }
                        />
                        <label class="form-check-label" for="bench-flash-attn">"Flash attention (-fa)"</label>
                    </div>
                    <small class="text-muted">"Default on. Disable to measure attention-kernel impact."</small>
                </div>
            </div>
        </section>

        // Presets — each preset maps to a phase of the tuning methodology.
        // Run them in order; re-run the KV-quant preset once per candidate
        // (q8_0, then q4_0) so you can read the delta at depth.
        <section class="card">
            <h3>"Methodology Presets"</h3>
            <small class="bench-hint">
                "Each preset loads the flags for one phase of the LLM tuning methodology. Run top-to-bottom, comparing results against the baseline."
            </small>
            <div class="preset-buttons">
                {BenchmarkPreset::all().into_iter().map(|preset| {
                    let desc = preset.description;
                    view! {
                        <button
                            class="btn btn-outline-secondary btn-sm"
                            title=desc
                            on:click=move |_| apply_preset_handler(preset.clone())
                        >
                            {preset.label}
                        </button>
                    }.into_any()
                }).collect::<Vec<_>>()}
            </div>
        </section>

        // Run button
        <div class="text-center my-3">
            <button
                class="btn btn-primary btn-lg"
                prop:disabled=move || selected_model_sig.get().is_empty() || is_running_sig.get()
                on:click=move |_| { submit_benchmark(); }
            >
                {move || if is_running_sig.get() { "Running..." } else { "▶ Run Benchmark" }}
            </button>
        </div>

        // Progress / logs — handled by JobLogPanel component.
        {move || {
            if let Some(job_id) = current_job_id_sig.get() {
                view! {
                    <JobLogPanel
                        job_id=job_id
                        on_result=on_result_cb
                        on_status=on_status_cb
                    />
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}

        // Benchmark results — single table with "t/s ± stddev" plus a header
        // card that surfaces the model metadata (backend, GPU, VRAM, load
        // time, batch/ubatch/KV choices) from the full BenchReport payload.
        {move || {
            let Some(report) = benchmark_results.get() else {
                return view! { <div></div> }.into_any();
            };

            // Accept either the full BenchReport shape or a bare summaries array
            // (legacy). Normalise to (summaries, model_info, vram, load_time, config).
            let (summaries, model_info, vram, load_time, config) = if let Some(arr) = report.as_array() {
                (arr.clone(), serde_json::Value::Null, serde_json::Value::Null, 0.0, serde_json::Value::Null)
            } else {
                let summaries = report.get("summaries")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let model_info = report.get("model_info").cloned().unwrap_or(serde_json::Value::Null);
                let vram = report.get("vram").cloned().unwrap_or(serde_json::Value::Null);
                let load_time = report.get("load_time_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let config = report.get("config").cloned().unwrap_or(serde_json::Value::Null);
                (summaries, model_info, vram, load_time, config)
            };

            if summaries.is_empty() {
                return view! { <div></div> }.into_any();
            }

            // Header card fields
            let mi_name = model_info.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mi_quant = model_info.get("quant").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mi_backend = model_info.get("backend").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mi_gpu = model_info.get("gpu_type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mi_ctx = model_info.get("context_length").and_then(|v| v.as_u64());
            let vram_used = vram.get("used_mib").and_then(|v| v.as_u64());
            let vram_total = vram.get("total_mib").and_then(|v| v.as_u64());
            let cfg_batch = config.get("batch_sizes").and_then(|v| v.as_array()).cloned();
            let cfg_ubatch = config.get("ubatch_sizes").and_then(|v| v.as_array()).cloned();
            let cfg_kv = config.get("kv_cache_type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let cfg_depth = config.get("depth").and_then(|v| v.as_array()).cloned();
            let cfg_fa = config.get("flash_attn").and_then(|v| v.as_bool());

            let has_header = !mi_name.is_empty();

            view! {
                <section class="card mt-3">
                    <h3>"Benchmark Results"</h3>

                    {if has_header {
                        view! {
                            <div class="bench-summary">
                                <div class="bench-summary__item">
                                    <div class="bench-summary__label">"Model"</div>
                                    <div class="bench-summary__value">{mi_name.clone()}</div>
                                </div>
                                {if !mi_quant.is_empty() {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"Quant"</div>
                                            <div class="bench-summary__value">{mi_quant}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                <div class="bench-summary__item">
                                    <div class="bench-summary__label">"Backend"</div>
                                    <div class="bench-summary__value">{format!("{} · {}", mi_backend, mi_gpu)}</div>
                                </div>
                                {if let (Some(used), Some(total)) = (vram_used, vram_total) {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"VRAM"</div>
                                            <div class="bench-summary__value">{format!("{} / {} MiB", used, total)}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(ctx) = mi_ctx {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"Context"</div>
                                            <div class="bench-summary__value">{ctx.to_string()}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                {if load_time > 0.0 {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"Load time"</div>
                                            <div class="bench-summary__value">{format!("{:.1} ms", load_time)}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(b) = cfg_batch.as_ref() {
                                    if !b.is_empty() {
                                        let s = b.iter().filter_map(|v| v.as_u64()).map(|v| v.to_string()).collect::<Vec<_>>().join(",");
                                        view! {
                                            <div class="bench-summary__item">
                                                <div class="bench-summary__label">"Batch"</div>
                                                <div class="bench-summary__value">{s}</div>
                                            </div>
                                        }.into_any()
                                    } else { view!{ <div></div> }.into_any() }
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(b) = cfg_ubatch.as_ref() {
                                    if !b.is_empty() {
                                        let s = b.iter().filter_map(|v| v.as_u64()).map(|v| v.to_string()).collect::<Vec<_>>().join(",");
                                        view! {
                                            <div class="bench-summary__item">
                                                <div class="bench-summary__label">"µ-batch"</div>
                                                <div class="bench-summary__value">{s}</div>
                                            </div>
                                        }.into_any()
                                    } else { view!{ <div></div> }.into_any() }
                                } else { view!{ <div></div> }.into_any() }}
                                {if !cfg_kv.is_empty() {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"KV cache"</div>
                                            <div class="bench-summary__value">{cfg_kv}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(b) = cfg_depth.as_ref() {
                                    if !b.is_empty() {
                                        let s = b.iter().filter_map(|v| v.as_u64()).map(|v| v.to_string()).collect::<Vec<_>>().join(",");
                                        view! {
                                            <div class="bench-summary__item">
                                                <div class="bench-summary__label">"Depth"</div>
                                                <div class="bench-summary__value">{s}</div>
                                            </div>
                                        }.into_any()
                                    } else { view!{ <div></div> }.into_any() }
                                } else { view!{ <div></div> }.into_any() }}
                                {if let Some(fa) = cfg_fa {
                                    view! {
                                        <div class="bench-summary__item">
                                            <div class="bench-summary__label">"Flash attn"</div>
                                            <div class="bench-summary__value">{if fa { "on" } else { "off" }}</div>
                                        </div>
                                    }.into_any()
                                } else { view!{ <div></div> }.into_any() }}
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }}

                    {render_summaries_table(&summaries)}
                </section>
            }.into_any()
        }}
                    }.into_any() // close inner view!{} for else branch (llama-bench form)
                }
                }}    // closes tab conditional closure

        // History — always shown. Newest rows appear at the top because the
        // server returns ORDER BY created_at DESC.
        <section class="card mt-3">
            <h3>"Benchmark History"</h3>
            {move || {
                let entries = history_sig.get();
                if entries.is_empty() {
                    view! {
                        <p class="text-muted">"No benchmarks yet. Run one above to see results here."</p>
                    }.into_any()
                } else {
                    view! {
                        <table class="table table-striped">
                            <thead>
                                <tr>
                                    <th style="width:1.5rem"></th>
                                    <th>"When"</th>
                                    <th>"Model"</th>
                                    <th>"Backend"</th>
                                    <th>"PP / TG sizes"</th>
                                    <th>"Best t/s"</th>
                                    <th>"Status"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {entries.into_iter().map(|entry| {
                                    let entry_id = entry.id;
                                    let when_title = format_timestamp(entry.created_at);
                                    let when_rel = format_relative(entry.created_at);
                                    let badge_class = if entry.status == "success" {
                                        "badge badge-success"
                                    } else {
                                        "badge badge-danger"
                                    };
                                    let name = entry.display_name.clone().unwrap_or_else(|| entry.model_id.clone());
                                    let quant_suffix = entry.quant
                                        .as_ref()
                                        .filter(|q| !q.is_empty())
                                        .map(|q| format!(" · {}", q))
                                        .unwrap_or_default();
                                    let model_cell = format!("{}{}", name, quant_suffix);

                                    let arr = entry.results.as_array();
                                    let best = |field: &str| -> String {
                                        arr.and_then(|items| {
                                            items.iter()
                                                .filter_map(|s| s.get(field).and_then(|v| v.as_f64()))
                                                .filter(|v| *v > 0.01)
                                                .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.max(v))))
                                        })
                                        .map(|v| format!("{v:.0}"))
                                        .unwrap_or_else(|| "—".to_string())
                                    };
                                    let best_cell = format!("PP {} · TG {}", best("pp_mean"), best("tg_mean"));

                                    let sizes = format!(
                                        "{} / {}",
                                        entry.pp_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
                                        entry.tg_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
                                    );

                                    // Reactive expansion state for this row. Leptos re-renders
                                    // just the chevron and the detail <tr> when the set flips.
                                    let is_open = Memo::new(move |_| expanded_sig.get().contains(&entry_id));
                                    let toggle = move |_| {
                                        expanded_history.update(|set| {
                                            if !set.insert(entry_id) { set.remove(&entry_id); }
                                        });
                                    };
                                    let summaries = entry.results.as_array().cloned().unwrap_or_default();
                                    let status_text = entry.status.clone();
                                    let backend_text = entry.backend.clone();
                                    let when_title_for_row = when_title.clone();

                                    view! {
                                        <tr class="bench-history__row" on:click=toggle>
                                            <td class="text-mono text-muted">{move || if is_open.get() { "▾" } else { "▸" }}</td>
                                            <td title=when_title_for_row>{when_rel}</td>
                                            <td>{model_cell}</td>
                                            <td><span class="badge badge-muted">{backend_text}</span></td>
                                            <td class="text-mono">{sizes}</td>
                                            <td class="text-mono">{best_cell}</td>
                                            <td><span class={badge_class}>{status_text}</span></td>
                                        </tr>
                                        {move || is_open.get().then(|| view! {
                                            <tr class="bench-history__detail">
                                                <td></td>
                                                <td colspan="6">
                                                    {render_summaries_table(&summaries)}
                                                </td>
                                            </tr>
                                        })}
                                    }.into_any()
                                }).collect::<Vec<_>>()}
                            </tbody>
                        </table>
                    }.into_any()
                }
            }}
        </section>
    }
}
