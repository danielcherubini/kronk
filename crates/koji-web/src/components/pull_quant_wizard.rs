use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ── Local types ──────────────────────────────────────────────────────────────

/// Mirrors `koji_core::config::QuantKind`. Used to distinguish model quants
/// from auxiliary files (mmproj) in the wizard's grouping logic.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
enum QuantKind {
    #[default]
    Model,
    Mmproj,
}

#[derive(Deserialize, Clone, Debug)]
struct QuantEntry {
    filename: String,
    quant: Option<String>,
    size_bytes: Option<i64>,
    #[serde(default)]
    kind: QuantKind,
}

#[derive(Clone, Debug)]
struct JobProgress {
    job_id: String,
    filename: String,
    status: String,
    bytes_downloaded: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
}

/// Returned by `POST /koji/v1/pulls` (each element of the array)
#[derive(Deserialize, Clone)]
struct PullJobEntry {
    job_id: String,
    filename: String,
    status: String,
}

/// SSE event data payload
#[derive(Deserialize, Clone)]
struct SsePayload {
    job_id: String,
    status: String,
    bytes_downloaded: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
}

// ── Wizard step enum ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum WizardStep {
    RepoInput,
    LoadingQuants,
    SelectQuants,
    Vision,
    SetContext,
    Downloading,
    Done,
}

// ── Helper ───────────────────────────────────────────────────────────────────

fn format_bytes(bytes: i64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{bytes} B")
    }
}

// ── Request body type ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct PullRequest {
    repo_id: String,
    quants: Vec<QuantRequest>,
}

#[derive(Serialize)]
struct QuantRequest {
    filename: String,
    quant: Option<String>,
    context_length: u32,
}

// ── Wizard step indicator helper ─────────────────────────────────────────────

fn step_class(current: &WizardStep, target: &WizardStep, target_idx: usize) -> &'static str {
    let order = [
        WizardStep::RepoInput,
        WizardStep::LoadingQuants,
        WizardStep::SelectQuants,
        WizardStep::SetContext,
        WizardStep::Downloading,
        WizardStep::Done,
    ];
    let current_idx = order.iter().position(|s| s == current).unwrap_or(0);
    if current == target {
        "wizard-step active"
    } else if current_idx > target_idx {
        "wizard-step completed"
    } else {
        "wizard-step"
    }
}

// ── Public types ─────────────────────────────────────────────────────────────

/// A quant that was successfully downloaded by the wizard. Emitted via the
/// `on_complete` callback so the host can merge new quants into its own state.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct CompletedQuant {
    /// Exactly the repo_id the wizard used (no -GGUF auto-resolution happens
    /// today; see the spec §12 for the pre-existing gap).
    pub repo_id: String,
    /// e.g. "Qwen3-8B-Q4_K_M.gguf"
    pub filename: String,
    /// e.g. "Q4_K_M". Built via the same three-step fallback as the backend's
    /// `_setup_model_after_pull_with_config`: the HF listing's quant label,
    /// else `infer_quant_from_filename`, else None (host falls back to the
    /// trimmed filename).
    pub quant: Option<String>,
    /// Final downloaded byte count. Sourced from the SSE done payload's
    /// `bytes_downloaded` field, which equals the actual on-disk file size
    /// because `download_chunked` writes bytes 1:1.
    ///
    /// Always `Some` today (`bytes_downloaded` is `u64`, never absent for a
    /// completed job). Wrapped in `Option` for forward-compat: a future
    /// backend revision that reports completion without a final byte count
    /// can set this to `None` and the editor's merge logic
    /// (`if cq.size_bytes.is_some() { ... }`) handles it correctly without
    /// clobbering an existing value.
    pub size_bytes: Option<u64>,
    /// Context length the user picked in step 4.
    pub context_length: u32,
}

// ── Local helper ─────────────────────────────────────────────────────────────

/// Local copy of `infer_quant_from_filename` from
/// `crates/koji-core/src/models/pull.rs` (around line 283). **MUST stay in
/// sync** with that function. Duplicated here because `koji-core` is only
/// available under the `ssr` feature and pulls in tokio/sqlite/reqwest, which
/// can't compile to WASM. If `koji-core` is later split into a WASM-compatible
/// utility crate, replace this with a direct import.
fn infer_quant_from_filename(filename: &str) -> Option<String> {
    let stem = filename.strip_suffix(".gguf")?;

    // Ordered longest-first so "Q4_K_M" matches before "Q4_K".
    let quant_patterns = [
        "IQ2_XXS", "IQ3_XXS", "IQ1_S", "IQ1_M", "IQ2_XS", "IQ2_S", "IQ2_M", "IQ3_XS", "IQ3_S",
        "IQ3_M", "IQ4_XS", "IQ4_NL", "Q2_K_S", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q4_K_S", "Q4_K_M",
        "Q4_K_L", "Q5_K_S", "Q5_K_M", "Q5_K_L", "Q2_K_XL", "Q3_K_XL", "Q4_K_XL", "Q5_K_XL",
        "Q6_K_XL", "Q8_K_XL", "Q2_K", "Q3_K", "Q4_K", "Q5_K", "Q6_K", "Q4_0", "Q4_1", "Q5_0",
        "Q5_1", "Q6_0", "Q8_0", "Q8_1", "F16", "F32", "BF16",
    ];

    let stem_upper = stem.to_uppercase();
    for pattern in &quant_patterns {
        if stem_upper.ends_with(pattern)
            || stem_upper.contains(&format!("-{}", pattern))
            || stem_upper.contains(&format!(".{}", pattern))
            || stem_upper.contains(&format!("_{}", pattern))
        {
            return Some(pattern.to_string());
        }
    }
    None
}

// ── Component ────────────────────────────────────────────────────────────────

#[component]
pub fn PullQuantWizard(
    /// Pre-set HF repo ID. If non-empty AND `is_open` transitions to true,
    /// the wizard skips step 1 and immediately fetches quants. If empty,
    /// the wizard starts at the repo-input step.
    #[prop(into)]
    initial_repo: Signal<String>,

    /// Whether the wizard is currently visible. Convention: `None` means
    /// "hosted directly on a page, always visible, never auto-reset" — the
    /// reset Effect is not registered. `Some(signal)` enables the modal
    /// lifecycle where (closed → open) transitions drive reset/refetch.
    #[prop(optional)]
    is_open: Option<Signal<bool>>,

    /// Called once after all downloads in the current session reach a terminal
    /// state. Receives the list of quants that completed successfully (failed
    /// jobs are filtered out). Fires exactly once per session, guarded by
    /// `did_complete`.
    #[prop(optional)]
    on_complete: Option<Callback<Vec<CompletedQuant>>>,

    /// Called when the user dismisses via in-step Cancel/Hide/Close button.
    /// Wizard never hides itself — host decides what happens.
    #[prop(optional)]
    on_close: Option<Callback<()>>,
) -> impl IntoView {
    // ── Signals ──────────────────────────────────────────────────────────────
    let wizard_step = RwSignal::new(WizardStep::RepoInput);
    let repo_id = RwSignal::new(String::new());
    let available_quants = RwSignal::new(Vec::<QuantEntry>::new());
    let available_mmprojs = RwSignal::new(Vec::<QuantEntry>::new());
    let selected_filenames = RwSignal::new(HashSet::<String>::new());
    let selected_mmproj_filenames = RwSignal::new(HashSet::<String>::new());
    let context_lengths = RwSignal::new(HashMap::<String, u32>::new());
    let download_jobs = RwSignal::new(Vec::<JobProgress>::new());
    let error_msg = RwSignal::new(Option::<String>::None);
    let did_complete = RwSignal::new(false);

    // ── Reset Effect (only if is_open is Some) ──────────────────────────────
    if let Some(is_open_sig) = is_open {
        Effect::new(move |_| {
            // Subscribe ONLY to is_open. Reading other signals tracked here
            // would race with the on_complete Effect on the Done transition.
            let open = is_open_sig.get();
            if !open {
                return;
            }
            let step = wizard_step.get_untracked();
            if !matches!(step, WizardStep::RepoInput | WizardStep::Done) {
                // Mid-flow session — preserve it across close/reopen.
                return;
            }
            // Always reset session state when (re)opening at a terminal step.
            selected_filenames.set(std::collections::HashSet::new());
            selected_mmproj_filenames.set(std::collections::HashSet::new());
            context_lengths.set(std::collections::HashMap::new());
            download_jobs.set(Vec::new());
            error_msg.set(None);
            did_complete.set(false);
            wizard_step.set(WizardStep::RepoInput);

            let repo = initial_repo.get_untracked();
            if repo.trim().is_empty() {
                return; // No auto-fetch for empty repo — user will type one in.
            }
            repo_id.set(repo.clone());
            wizard_step.set(WizardStep::LoadingQuants);

            // Spawn the same fetch the Search button does today.
            wasm_bindgen_futures::spawn_local(async move {
                let url = format!("/koji/v1/hf/{}", repo);
                match gloo_net::http::Request::get(&url).send().await {
                    Ok(resp) => match resp.json::<Vec<QuantEntry>>().await {
                        Ok(quants) => {
                            if quants.is_empty() {
                                error_msg.set(Some(
                                    "No GGUF files found for this repo. Check the repo ID and try again.".to_string(),
                                ));
                                wizard_step.set(WizardStep::RepoInput);
                            } else {
                                // Separate quants from mmprojs
                                let mut model_quants: Vec<QuantEntry> = Vec::new();
                                let mut mmprojs: Vec<QuantEntry> = Vec::new();
                                for q in quants {
                                    if q.kind == QuantKind::Mmproj {
                                        mmprojs.push(q);
                                    } else {
                                        model_quants.push(q);
                                    }
                                }
                                available_quants.set(model_quants);
                                available_mmprojs.set(mmprojs);
                                wizard_step.set(WizardStep::SelectQuants);
                            }
                        }
                        Err(e) => {
                            error_msg.set(Some(format!("Failed to parse response: {e}")));
                            wizard_step.set(WizardStep::RepoInput);
                        }
                    },
                    Err(e) => {
                        error_msg.set(Some(format!("Request failed: {e}")));
                        wizard_step.set(WizardStep::RepoInput);
                    }
                }
            });
        });
    }

    // ── on_complete Effect (only if on_complete is Some) ─────────────────────
    if let Some(cb) = on_complete {
        Effect::new(move |_| {
            let step = wizard_step.get();
            if step != WizardStep::Done {
                return;
            }
            if did_complete.get_untracked() {
                return;
            }
            did_complete.set(true);

            let jobs = download_jobs.get_untracked();
            let quants_listing = available_quants.get_untracked();
            let ctx_map = context_lengths.get_untracked();
            let repo = repo_id.get_untracked();

            let completed: Vec<CompletedQuant> = jobs
                .into_iter()
                .filter(|j| j.status == "completed")
                .map(|j| {
                    let entry = quants_listing.iter().find(|q| q.filename == j.filename);
                    let quant = entry
                        .and_then(|e| e.quant.clone())
                        .or_else(|| infer_quant_from_filename(&j.filename));
                    let context_length = ctx_map.get(&j.filename).copied().unwrap_or(32768);
                    CompletedQuant {
                        repo_id: repo.clone(),
                        filename: j.filename.clone(),
                        quant,
                        // Always Some today — `bytes_downloaded` is u64, never None.
                        // Wrapped in Option for forward-compat (e.g. if a future
                        // backend reports completion without a final byte count).
                        size_bytes: Some(j.bytes_downloaded),
                        context_length,
                    }
                })
                .collect();

            cb.run(completed);
        });
    }

    // ── Step dispatch ───────────────────────────────────────────────────────
    view! {
        // Wizard step indicator
        <div class="wizard-steps mb-3">
            {move || {
                let step = wizard_step.get();
                let show_repo_step = initial_repo.get().trim().is_empty();
                view! {
                    {show_repo_step.then(|| view! {
                        <div class=step_class(&step, &WizardStep::RepoInput, 0)>
                            "1. Repo"
                        </div>
                    })}
                    <div class=step_class(&step, &WizardStep::LoadingQuants, 1)>
                        "2. Loading"
                    </div>
                    <div class=step_class(&step, &WizardStep::SelectQuants, 2)>
                        "3. Select"
                    </div>
                    <div class=step_class(&step, &WizardStep::Vision, 3)>
                        "4. Vision"
                    </div>
                    <div class=step_class(&step, &WizardStep::SetContext, 4)>
                        "5. Context"
                    </div>
                    <div class=step_class(&step, &WizardStep::Downloading, 5)>
                        "6. Download"
                    </div>
                    <div class=step_class(&step, &WizardStep::Done, 6)>
                        "7. Done"
                    </div>
                }
            }}
        </div>

        <div class="card">
            {move || match wizard_step.get() {
                // ── Step 1: Repo Input ────────────────────────────────────────
                WizardStep::RepoInput => view! {
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
                                if rid.is_empty() { return; }
                                // Clear any stale per-repo state from a previous search.
                                error_msg.set(None);
                                selected_filenames.set(std::collections::HashSet::new());
                                context_lengths.set(std::collections::HashMap::new());
                                available_quants.set(Vec::new());
                                wizard_step.set(WizardStep::LoadingQuants);
                                wasm_bindgen_futures::spawn_local(async move {
                                    let url = format!("/koji/v1/hf/{}", rid);
                                    match gloo_net::http::Request::get(&url).send().await {
                                        Ok(resp) => {
                                            match resp.json::<Vec<QuantEntry>>().await {
                                                Ok(quants) => {
                                                    if quants.is_empty() {
                                                        error_msg.set(Some(
                                                            "No GGUF files found for this repo. Check the repo ID and try again.".to_string()
                                                        ));
                                                        wizard_step.set(WizardStep::RepoInput);
                                                    } else {
                                                        // Separate quants from mmprojs
                                                        let mut model_quants: Vec<QuantEntry> = Vec::new();
                                                        let mut mmprojs: Vec<QuantEntry> = Vec::new();
                                                        for q in quants {
                                                            if q.kind == QuantKind::Mmproj {
                                                                mmprojs.push(q);
                                                            } else {
                                                                model_quants.push(q);
                                                            }
                                                        }
                                                        available_quants.set(model_quants);
                                                        available_mmprojs.set(mmprojs);
                                                        wizard_step.set(WizardStep::SelectQuants);
                                                    }
                                                }
                                                Err(e) => {
                                                    error_msg.set(Some(format!("Failed to parse response: {e}")));
                                                    wizard_step.set(WizardStep::RepoInput);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error_msg.set(Some(format!("Request failed: {e}")));
                                            wizard_step.set(WizardStep::RepoInput);
                                        }
                                    }
                                });
                            }
                        >"Search"</button>
                    </div>
                }.into_any(),

                // ── Step 2: Loading ───────────────────────────────────────────
                WizardStep::LoadingQuants => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"Searching HuggingFace..."</h2>
                    </div>
                    <div class="spinner-container">
                        <span class="spinner"></span>
                        <span class="text-muted">"Fetching available quants, please wait..."</span>
                    </div>
                }.into_any(),

                // ── Step 3: Select Quants ─────────────────────────────────────
                WizardStep::SelectQuants => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"Select Quantisations"</h2>
                        <p class="form-card__desc text-muted">
                            "Choose one or more quantisation files to download from "
                            <code>{move || repo_id.get()}</code>"."
                        </p>
                    </div>

                    <div class="form-actions mb-2">
                        <button class="btn btn-secondary btn-sm" on:click=move |_| {
                            let all: HashSet<String> = available_quants
                                .get()
                                .iter()
                                .map(|q| q.filename.clone())
                                .collect();
                            selected_filenames.set(all);
                        }>
                            "Select All"
                        </button>
                        <button class="btn btn-secondary btn-sm" on:click=move |_| {
                            selected_filenames.set(HashSet::new());
                        }>
                            "Deselect All"
                        </button>
                    </div>

                    <table class="data-table">
                        <thead>
                            <tr>
                                <th class="icon-sm"></th>
                                <th>"Quant"</th>
                                <th>"Filename"</th>
                                <th>"Size"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || available_quants.get().into_iter().map(|q| {
                                let fname = q.filename.clone();
                                let fname_check = fname.clone();
                                let label = q.quant.clone().unwrap_or_else(|| fname.clone());
                                let size_str = q.size_bytes
                                    .map(format_bytes)
                                    .unwrap_or_else(|| "?".to_string());
                                let is_checked = move || selected_filenames.get().contains(&fname_check);
                                view! {
                                    <tr>
                                        <td>
                                            <input
                                                type="checkbox"
                                                prop:checked=is_checked
                                                on:change=move |_| {
                                                    selected_filenames.update(|set| {
                                                        if set.contains(&fname) {
                                                            set.remove(&fname);
                                                        } else {
                                                            set.insert(fname.clone());
                                                        }
                                                    });
                                                }
                                            />
                                        </td>
                                        <td>
                                            <span class="badge badge-info">{label}</span>
                                        </td>
                                        <td><code>{q.filename.clone()}</code></td>
                                        <td class="text-muted">{size_str}</td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()}
                        </tbody>
                    </table>

                    <div class="form-actions mt-3">
                        <Show when=move || initial_repo.get().trim().is_empty()>
                            <button class="btn btn-secondary" on:click=move |_| {
                                wizard_step.set(WizardStep::RepoInput);
                            }>
                                "Back"
                            </button>
                        </Show>
                        <button
                            class="btn btn-primary"
                            prop:disabled=move || selected_filenames.get().is_empty()
                            on:click=move |_| {
                                // Pre-fill context_lengths with 32768 for each selected filename
                                let sel = selected_filenames.get();
                                let mut ctx = HashMap::new();
                                for fname in &sel {
                                    ctx.insert(fname.clone(), 32768u32);
                                }
                                context_lengths.set(ctx);
                                // If mmprojs exist, go to Vision step first
                                if !available_mmprojs.get().is_empty() {
                                    wizard_step.set(WizardStep::Vision);
                                } else {
                                    wizard_step.set(WizardStep::SetContext);
                                }
                            }
                        >"Next →"</button>
                    </div>
                }.into_any(),

                // ── Step 4: Vision ────────────────────────────────────────────
                WizardStep::Vision => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"Select Vision Projector"</h2>
                        <p class="form-card__desc text-muted">
                            "Choose a vision projector file for multimodal support."
                        </p>
                    </div>

                    <div class="form-group">
                        <label class="form-label" for="mmproj-select">"Vision Projector"</label>
                        <select
                            id="mmproj-select"
                            class="form-select"
                            multiple
                            on:change=move |e| {
                                use wasm_bindgen::JsCast;
                                let select = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap();
                                let selected_filenames: HashSet<String> = {
                                    let options = select.selected_options();
                                    let len = options.length();
                                    let mut set = HashSet::new();
                                    for i in 0..len {
                                        if let Some(option) = options.item(i) {
                                            if let Some(value) = option.get_attribute("value") {
                                                set.insert(value);
                                            }
                                        }
                                    }
                                    set
                                };
                                selected_mmproj_filenames.set(selected_filenames);
                            }
                        >
                            {move || available_mmprojs.get().into_iter().map(|m| {
                                let filename = m.filename.clone();
                                view! { <option value=filename.clone()>{filename.clone()}</option> }
                            }).collect::<Vec<_>>()}
                        </select>
                        <span class="form-hint">"Hold Ctrl/Cmd to select multiple"</span>
                    </div>

                    {move || {
                        if available_mmprojs.get().is_empty() {
                            None
                        } else {
                            Some(view! {
                                <div class="alert alert--info mt-2">
                                    <span class="alert__icon">"ℹ"</span>
                                    <span>"Vision projector available: " {available_mmprojs.get().len()} " file(s) found"</span>
                                </div>
                            }.into_any())
                        }
                    }}

                    <div class="form-actions mt-3">
                        <button class="btn btn-secondary" on:click=move |_| {
                            wizard_step.set(WizardStep::SelectQuants);
                        }>
                            "Back"
                        </button>
                        <button
                            class="btn btn-primary"
                            prop:disabled=move || selected_mmproj_filenames.get().is_empty()
                            on:click=move |_| {
                                wizard_step.set(WizardStep::SetContext);
                            }
                        >"Next →"</button>
                    </div>
                }.into_any(),

                // ── Step 5: Set Context ────────────────────────────────────────
                WizardStep::SetContext => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"Set Context Length"</h2>
                        <p class="form-card__desc text-muted">
                            "Configure the context window size for each selected quantisation."
                        </p>
                    </div>

                    <table class="data-table">
                        <thead>
                            <tr>
                                <th>"Quant"</th>
                                <th>"Filename"</th>
                                <th>"Context Length"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let sel = selected_filenames.get();
                                available_quants.get().into_iter()
                                    .filter(|q| sel.contains(&q.filename))
                                    .map(|q| {
                                        let fname = q.filename.clone();
                                        let fname_input = fname.clone();
                                        let label = q.quant.clone().unwrap_or_else(|| fname.clone());
                                        let current_ctx = move || context_lengths.get()
                                            .get(&fname_input)
                                            .copied()
                                            .unwrap_or(32768);
                                        view! {
                                            <tr>
                                                <td>
                                                    <span class="badge badge-info">{label}</span>
                                                </td>
                                                <td><code>{q.filename.clone()}</code></td>
                                                <td>
                                                    <input
                                                        class="form-input input-narrow"
                                                        type="number"
                                                        min="512"
                                                        step="512"
                                                        prop:value=current_ctx
                                                        on:change=move |e| {
                                                            if let Ok(v) = event_target_value(&e).parse::<u32>() {
                                                                context_lengths.update(|m| {
                                                                    m.insert(fname.clone(), v);
                                                                });
                                                            }
                                                        }
                                                    />
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()
                            }}
                        </tbody>
                    </table>

                    <div class="form-actions mt-3">
                        <button class="btn btn-secondary" on:click=move |_| {
                            // If mmprojs exist, go to Vision step; otherwise go to SelectQuants
                            if !available_mmprojs.get().is_empty() {
                                wizard_step.set(WizardStep::Vision);
                            } else {
                                wizard_step.set(WizardStep::SelectQuants);
                            }
                        }>
                            "Back"
                        </button>
                        <button class="btn btn-primary" on:click=move |_| {
                            let rid = repo_id.get();
                            let sel = selected_filenames.get();
                            let quants_list = available_quants.get();
                            let ctx_map = context_lengths.get();

                            // Build request payload
                            let mut quants: Vec<QuantRequest> = sel.iter()
                                .filter_map(|fname| {
                                    let entry = quants_list.iter().find(|q| &q.filename == fname)?;
                                    let ctx = ctx_map.get(fname).copied().unwrap_or(32768);
                                    Some(QuantRequest {
                                        filename: fname.clone(),
                                        quant: entry.quant.clone(),
                                        context_length: ctx,
                                    })
                                })
                                .collect();

                            // Add selected mmprojs (no context length needed for mmprojs)
                            let available_mmprojs_list = available_mmprojs.get();
                            let selected_mmprojs: Vec<QuantRequest> = selected_mmproj_filenames
                                .get()
                                .iter()
                                .filter_map(|fname| {
                                    let entry = available_mmprojs_list.iter().find(|q| &q.filename == fname)?;
                                    Some(QuantRequest {
                                        filename: fname.clone(),
                                        quant: entry.quant.clone(),
                                        context_length: 32768,  // mmprojs don't need context length, use default
                                    })
                                })
                                .collect();

                            quants.extend(selected_mmprojs);

                            let body = PullRequest { repo_id: rid, quants };

                            wasm_bindgen_futures::spawn_local(async move {
                                let build_result = gloo_net::http::Request::post("/koji/v1/pulls")
                                    .json(&body);
                                let resp = match build_result {
                                    Ok(req) => req.send().await,
                                    Err(e) => {
                                        error_msg.set(Some(format!("Failed to build request: {e}")));
                                        return;
                                    }
                                };
                                match resp {
                                    Ok(r) => {
                                        match r.json::<Vec<PullJobEntry>>().await {
                                            Ok(entries) => {
                                                let jobs: Vec<JobProgress> = entries
                                                    .iter()
                                                    .map(|e| JobProgress {
                                                        job_id: e.job_id.clone(),
                                                        filename: e.filename.clone(),
                                                        status: e.status.clone(),
                                                        bytes_downloaded: 0,
                                                        total_bytes: None,
                                                        error: None,
                                                    })
                                                    .collect();
                                                download_jobs.set(jobs);
                                                wizard_step.set(WizardStep::Downloading);

                                                // Open SSE stream for each job
                                                for entry in entries {
                                                    let job_id_str = entry.job_id.clone();
                                                    let dj = download_jobs;
                                                    let ws = wizard_step;
                                                    wasm_bindgen_futures::spawn_local(async move {
                                                        let url = format!("/koji/v1/pulls/{}/stream", job_id_str);
                                                        // Helper: check if all jobs have finished and advance wizard.
                                                        let advance_if_done = |dj: RwSignal<Vec<JobProgress>>, ws: RwSignal<WizardStep>| {
                                                            let jobs = dj.get();
                                                            if !jobs.is_empty() && jobs.iter().all(|j| {
                                                                j.status == "completed" || j.status == "failed"
                                                            }) {
                                                                ws.set(WizardStep::Done);
                                                            }
                                                        };

                                                        let mut es = match EventSource::new(&url) {
                                                            Ok(es) => es,
                                                            Err(e) => {
                                                                let msg = format!("{e:?}");
                                                                dj.update(|jobs| {
                                                                    if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job_id_str) {
                                                                        j.status = "failed".to_string();
                                                                        j.error = Some(format!("Failed to open SSE stream: {msg}"));
                                                                    }
                                                                });
                                                                advance_if_done(dj, ws);
                                                                return;
                                                            }
                                                        };
                                                        let mut progress_stream = match es.subscribe("progress") {
                                                            Ok(s) => s,
                                                            Err(e) => {
                                                                let msg = format!("{e:?}");
                                                                dj.update(|jobs| {
                                                                    if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job_id_str) {
                                                                        j.status = "failed".to_string();
                                                                        j.error = Some(format!("Failed to subscribe to progress events: {msg}"));
                                                                    }
                                                                });
                                                                es.close();
                                                                advance_if_done(dj, ws);
                                                                return;
                                                            }
                                                        };
                                                        let mut done_stream = match es.subscribe("done") {
                                                            Ok(s) => s,
                                                            Err(e) => {
                                                                let msg = format!("{e:?}");
                                                                dj.update(|jobs| {
                                                                    if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job_id_str) {
                                                                        j.status = "failed".to_string();
                                                                        j.error = Some(format!("Failed to subscribe to done events: {msg}"));
                                                                    }
                                                                });
                                                                es.close();
                                                                advance_if_done(dj, ws);
                                                                return;
                                                            }
                                                        };

                                                        loop {
                                                            let next_progress = progress_stream.next();
                                                            let next_done = done_stream.next();
                                                            futures_util::pin_mut!(next_progress, next_done);

                                                            match futures_util::future::select(next_progress, next_done).await {
                                                                futures_util::future::Either::Left((Some(Ok((_, msg))), _)) => {
                                                                    let data = msg.data().as_string().unwrap_or_default();
                                                                    if let Ok(p) = serde_json::from_str::<SsePayload>(&data) {
                                                                        dj.update(|jobs| {
                                                                            if let Some(j) = jobs.iter_mut().find(|j| j.job_id == p.job_id) {
                                                                                j.bytes_downloaded = p.bytes_downloaded;
                                                                                j.total_bytes = p.total_bytes;
                                                                                j.status = p.status.clone();
                                                                                j.error = p.error.clone();
                                                                            }
                                                                        });
                                                                    }
                                                                }
                                                                futures_util::future::Either::Right((Some(Ok((_, msg))), _)) => {
                                                                    let data = msg.data().as_string().unwrap_or_default();
                                                                    if let Ok(p) = serde_json::from_str::<SsePayload>(&data) {
                                                                        dj.update(|jobs| {
                                                                            if let Some(j) = jobs.iter_mut().find(|j| j.job_id == p.job_id) {
                                                                                j.bytes_downloaded = p.bytes_downloaded;
                                                                                j.total_bytes = p.total_bytes;
                                                                                j.status = p.status.clone();
                                                                                j.error = p.error.clone();
                                                                            }
                                                                        });
                                                                    }
                                                                    es.close();
                                                                    advance_if_done(dj, ws);
                                                                    break;
                                                                }
                                                                // Stream ended unexpectedly — close and check if all jobs done.
                                                                _ => {
                                                                    es.close();
                                                                    advance_if_done(dj, ws);
                                                                    break;
                                                                }
                                                            }
                                                        }
                                                    });
                                                }
                                            }
                                            Err(e) => {
                                                error_msg.set(Some(format!("Failed to parse response: {e}")));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error_msg.set(Some(format!("Request failed: {e}")));
                                    }
                                }
                            });
                        }>
                            "Start Download"
                        </button>
                        {on_close.map(|cb| view! {
                            <button type="button" class="btn btn-secondary" on:click=move |_| cb.run(())>
                                "Cancel"
                            </button>
                        })}
                    </div>
                }.into_any(),

                // ── Step 5: Downloading ───────────────────────────────────────
                WizardStep::Downloading => view! {
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
                }.into_any(),

                // ── Step 6: Done ──────────────────────────────────────────────
                WizardStep::Done => view! {
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
                }.into_any(),
            }}
        </div>
    }
}
