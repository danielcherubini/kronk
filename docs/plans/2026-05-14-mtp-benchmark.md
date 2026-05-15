# MTP Benchmark Plan

**Goal:** Add an "MTP Testing" tab to the Benchmarks page that sweeps `--spec-draft-n-max` (0-8) with `--spec-type draft-mtp`, running 9 diverse prompts against a dedicated llama-server per config to measure throughput, draft acceptance, and wall time.

**Architecture:** New `llama_cli_mtp` module in `tama-core` reuses the shared `ServerHandle` from `llama_cli_spec::server` (extended with a `chat_complete` method). Dedicated API endpoint `/tama/v1/benchmarks/mtp-run` follows the existing job+SSE pattern. New web UI tab component in `tama-web`.

**Tech Stack:** Rust (tama-core, tama-web), Leptos (web UI), reqwest (HTTP), llama-server (child process)

---

### Task 1: Add `chat_complete` to `ServerHandle`, `DraftMtp` to `SpecType`, and `spec_draft_ngl` to `ServerArgs`

**Context:**
The existing `ServerHandle` in `llama_cli_spec/server.rs` has a `complete` method that hits `/v1/completions` (legacy prompt endpoint). MTP benchmarking needs `/v1/chat/completions` (messages-based endpoint). Additionally, `SpecType` currently only has n-gram variants — we need `DraftMtp` for the `--spec-type draft-mtp` flag. And `ServerArgs` is missing `spec_draft_ngl` needed for `--spec-draft-ngl 99`. All three changes are prerequisites for the MTP runner in Task 2.

**Files:**
- Modify: `crates/tama-core/src/bench/llama_cli_spec/server.rs` (chat_complete, ServerArgs)
- Modify: `crates/tama-core/src/bench/llama_cli_spec/mod.rs` (SpecType::DraftMtp)

**What to implement:**

1. Add `DraftMtp` variant to `SpecType` enum in `mod.rs`:
```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum SpecType {
    NgramSimple,
    NgramMod,
    NgramMapK,
    NgramMapK4v,
    DraftMtp,  // NEW
}
```
- `as_str()` returns `"draft-mtp"`
- `spec_ngram_flags()` returns `("", "", "")` (MTP has no n-gram flags)
- In `server.rs::to_args()`, guard ngram flag emission: only emit when the flag strings are non-empty (so DraftMtp doesn't emit empty flags)

2. Add `spec_draft_ngl: Option<u32>` to `ServerArgs` in `server.rs`:
```rust
pub struct ServerArgs {
    // ... existing fields ...
    pub spec_draft_ngl: Option<u32>,  // NEW: --spec-draft-ngl
}
```
- In `to_args()`, emit `--spec-draft-ngl <val>` when `spec_draft_ngl.is_some()` and `spec_type.is_some()`

3. Add a `ChatTiming` struct to `server.rs`:
```rust
/// Timing and usage data from a chat completion response.
#[derive(Debug, Clone)]
pub struct ChatTiming {
    pub predicted_per_second: f64,
    pub predicted_n: u32,       // completion_tokens
    pub draft_n: u32,           // total draft tokens proposed
    pub draft_n_accepted: u32,  // draft tokens accepted
}
```

4. Add a `chat_complete` method to `ServerHandle`:
```rust
pub async fn chat_complete(
    &self,
    model: &str,
    messages: &[(&str, &str)],  // [(role, content)]
    max_tokens: u32,
) -> Result<ChatTiming>
```

- POST to `{base_url}/v1/chat/completions`
- Request body: `{ "model": "<model>", "messages": [...], "max_tokens": 192, "seed": 42 }`
- Parse response for `timings.predicted_per_second`, `timings.draft_n`, `timings.draft_n_accepted`, `usage.completion_tokens`
- Use `reqwest::Client` with 600s timeout (same as existing `complete` method)
- The `messages` parameter takes `&[(&str, &str)]` where each tuple is `(role, content)` — roles are "user", "assistant", "system"

5. Add `#[derive(serde::Deserialize)]` structs for parsing the chat completion response internally within the method (same pattern as existing `complete` method's `CompletionResponse`/`Timings`).

**Steps:**
- [ ] Add `DraftMtp` variant to `SpecType` enum in `mod.rs` with `as_str() => "draft-mtp"` and `spec_ngram_flags() => ("", "", "")`
- [ ] Guard ngram flag emission in `server.rs::to_args()`: only emit ngram flags when flag strings are non-empty
- [ ] Add `spec_draft_ngl: Option<u32>` to `ServerArgs` and plumb through `to_args()`
- [ ] Add `ChatTiming` struct to `server.rs`
- [ ] Add `chat_complete` method to `ServerHandle`
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix warnings.
- [ ] Commit with message: "feat: add chat_complete, DraftMtp spec type, and spec_draft_ngl for MTP benchmarking"

**Acceptance criteria:**
- [ ] `SpecType::DraftMtp` exists with `as_str() == "draft-mtp"` and empty `spec_ngram_flags()`
- [ ] `ServerArgs` has `spec_draft_ngl: Option<u32>` field emitted in `to_args()`
- [ ] `ServerHandle::chat_complete` compiles and accepts `&[(&str, &str)]` messages
- [ ] `ChatTiming` struct has all 4 fields: `predicted_per_second`, `predicted_n`, `draft_n`, `draft_n_accepted`
- [ ] `to_args()` doesn't emit empty ngram flags for `DraftMtp`
- [ ] No clippy warnings

---

### Task 2: Create `llama_cli_mtp` core module

**Context:**
This is the core MTP benchmarking logic. It embeds the 9 prompts from `mtp-bench.py`, spawns a llama-server per draft-n-max config, runs all 9 prompts via chat_complete, and collects per-prompt + aggregate metrics. The module reuses `ServerHandle` and `spawn_server` from `llama_cli_spec::server`, plus `find_llama_server` from `llama_cli_spec::discovery`.

**Files:**
- Create: `crates/tama-core/src/bench/llama_cli_mtp/mod.rs`
- Modify: `crates/tama-core/src/bench/mod.rs` (add `pub mod llama_cli_mtp;`)

**What to implement:**

1. Create `crates/tama-core/src/bench/llama_cli_mtp/mod.rs` with:

**Constants — the 9 prompts from mtp-bench.py:**
```rust
pub const MTP_PROMPTS: &[(&str, &str)] = &[
    ("code_python", "Write a Python function that returns the n-th Fibonacci number using memoization. Include a docstring."),
    ("code_cpp", "Write a C++ template function `clamp(x, lo, hi)` that returns x clamped to [lo, hi]. No std::clamp."),
    ("explain_concept", "Explain how speculative decoding works in large language model inference, in three short paragraphs."),
    ("summarize", "Summarize in two sentences: The Industrial Revolution began in Britain in the late 18th century, transforming manufacturing through mechanization, steam power, and the factory system. It spread to continental Europe and North America during the 19th century."),
    ("qa_factual", "Q: What are the four fundamental forces of physics?\nA:"),
    ("translation", "Translate to French: 'The quick brown fox jumps over the lazy dog.'"),
    ("creative_short", "Write a four-line poem about an old lighthouse."),
    ("stepwise_math", "Solve step by step: A train leaves station A at 60 km/h. Two hours later, a second train leaves the same station on the same track at 90 km/h. How long until the second train catches the first?"),
    ("long_code_review", "<the full long code review prompt from mtp-bench.py>"),
];
```

**Types:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpBenchConfig {
    pub model_path: PathBuf,
    pub draft_max_values: Vec<u32>,
    pub ngl: Option<u32>,
    pub draft_ngl: Option<u32>,   // --spec-draft-ngl (default Some(99))
    pub flash_attn: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpPromptResult {
    pub draft_max: u32,           // which draft-n-max config produced this result
    pub name: String,
    pub wall_s: f64,
    pub predicted_n: u32,
    pub draft_n: u32,
    pub draft_n_accepted: u32,
    pub accept_rate: Option<f64>, // None when draft_n == 0 (baseline), else accepted/draft
    pub predicted_per_second: f64,
    pub error: Option<String>,    // Set if this prompt failed; all numeric fields = 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpBenchResult {
    pub entries: Vec<MtpPromptResult>,
    pub aggregate: MtpAggregate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpAggregate {
    pub n_requests: usize,
    pub total_predicted: u32,
    pub total_draft: u32,
    pub total_draft_accepted: u32,
    pub aggregate_accept_rate: f64, // total_draft_accepted / total_draft; 0.0 if total_draft == 0
    pub wall_s_total: f64,
}
```

**Server args construction:**
Use `ServerArgs` from `llama_cli_spec::server` with:
- `spec_type: Some(SpecType::DraftMtp)`
- `draft_max: Some(N)` where N is the current draft-n-max value
- `draft_min: None` (not used for MTP)
- `spec_draft_ngl: config.draft_ngl` (default Some(99))
- `ngl: config.ngl`
- `flash_attn: config.flash_attn`
- All ngram fields: `None` (not used for MTP)

The `ServerArgs::to_args()` method handles emitting all flags including `--spec-type draft-mtp`, `--spec-draft-n-max <N>`, `--spec-draft-ngl 99`.

**Runner function:**
```rust
pub async fn run_mtp_bench(
    config: &MtpBenchConfig,
    binary_override: Option<PathBuf>,
    progress: Arc<dyn ProgressSink>,
) -> Result<MtpBenchResult>
```

Runner logic:
1. Discover llama-server binary (use `find_llama_server` from `llama_cli_spec`, searching in parent dir of model_path)
2. Extract model name: `model_path.file_stem().unwrap_or(std::ffi::OsStr::new("model")).to_string_lossy().into_owned()` — passed to `chat_complete` as the model identifier
3. **Baseline phase** (draft-n-max=0): Find available port, build `ServerArgs` with `draft_max: Some(0)`, spawn server via `spawn_server`, wait ready. For each of the 9 prompts: time the request, call `chat_complete`, collect `ChatTiming` + wall time, build `MtpPromptResult` with `draft_max: 0`. Kill server, 2s sleep.
4. **Sweep phase**: For each `draft_max` in `config.draft_max_values` where `draft_max > 0`: same as baseline but with the given draft_max value. 2s sleep between configs.
5. Build `MtpBenchResult`: entries = all `MtpPromptResult` in order, aggregate = computed from all entries.
6. Call `progress.result()` with the serialized `MtpBenchResult` JSON before returning.

**Per-prompt error handling:**
- Wrap each `chat_complete` call in a match. On `Err(e)`, log the error via `progress.log()`, create `MtpPromptResult` with `error: Some(e.to_string())`, all numeric fields = 0, `accept_rate: None`, and continue to the next prompt
- One bad prompt should NOT crash the entire sweep

**Accept rate computation:**
```rust
let accept_rate = if timing.draft_n > 0 {
    Some(timing.draft_n_accepted as f64 / timing.draft_n as f64)
} else {
    None  // baseline (draft_max=0): no drafts proposed, no rate
};
```

**Aggregate accept rate:**
```rust
let aggregate_accept_rate = if total_draft > 0 {
    total_draft_accepted as f64 / total_draft as f64
} else {
    0.0
};
```

**Key details:**
- `max_tokens` is always 192 (matching mtp-bench.py)
- `seed` is always 42
- `ngl` defaults to `Some(99)` if not set in config
- `draft_ngl` defaults to `Some(99)` if not set in config
- `flash_attn` defaults to `true`
- Use `use crate::bench::llama_cli_spec::server::{self, ServerHandle, ServerArgs};` and `use crate::bench::llama_cli_spec::{find_llama_server, SpecType};`

2. Add `pub mod llama_cli_mtp;` to `crates/tama-core/src/bench/mod.rs`.

**Steps:**
- [ ] Create `crates/tama-core/src/bench/llama_cli_mtp/mod.rs` with all types, constants, and runner
- [ ] Add `pub mod llama_cli_mtp;` to `crates/tama-core/src/bench/mod.rs`
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: add llama_cli_mtp module for MTP benchmarking"

**Acceptance criteria:**
- [ ] `MtpBenchConfig`, `MtpPromptResult`, `MtpBenchResult`, `MtpAggregate` all derive `Debug, Clone, Serialize, Deserialize`
- [ ] `MTP_PROMPTS` contains all 9 prompts from mtp-bench.py
- [ ] `run_mtp_bench` compiles, runs baseline (draft=0) then sweep, collects per-prompt results
- [ ] No clippy warnings

---

### Task 3: Create MTP benchmark API endpoint

**Context:**
The web UI needs an API endpoint to trigger MTP benchmarks. This follows the exact same pattern as the existing spec benchmark endpoint (`/tama/v1/benchmarks/spec-run`): submit a job, spawn background task, stream progress via SSE, store results in DB. The endpoint accepts model selection, backend selection, and draft-n-max sweep range.

**Files:**
- Create: `crates/tama-web/src/api/benchmarks/mtp.rs`
- Modify: `crates/tama-web/src/api/benchmarks/mod.rs` (add mtp module + re-exports)
- Modify: `crates/tama-web/src/server.rs` (register route)

**What to implement:**

1. Create `crates/tama-web/src/api/benchmarks/mtp.rs`:

**Request DTO:**
```rust
#[derive(Debug, Clone, Deserialize)]
pub struct MtpBenchmarkRunRequest {
    pub model_id: String,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub backend_name: Option<String>,
    #[serde(default)]
    pub gpu_variant: Option<String>,
    #[serde(default = "default_draft_max_values")]
    pub draft_max_values: Vec<u32>,
    #[serde(default = "default_ngl")]
    pub ngl: Option<u32>,
    #[serde(default = "default_draft_ngl")]
    pub draft_ngl: Option<u32>,
    #[serde(default = "default_flash_attn")]
    pub flash_attn: bool,
    #[serde(default)]
    pub benchmark_type: Option<String>,
}
```
- `default_draft_max_values` returns `vec![0, 1, 2, 3, 4, 5, 6, 7, 8]`
- `default_ngl` returns `Some(99)`
- `default_draft_ngl` returns `Some(99)`
- `default_flash_attn` returns `true`

**Handler — `run_mtp_benchmark`:**
```rust
pub async fn run_mtp_benchmark(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MtpBenchmarkRunRequest>,
) -> impl IntoResponse
```
- Check job manager availability (same pattern as spec.rs)
- Validate `draft_max_values` is not empty
- Submit `JobKind::Benchmark` job
- Spawn background task calling `run_mtp_benchmark_inner`
- Return `(StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id }))`

**Inner function — `run_mtp_benchmark_inner`:**
Follow the pattern from `spec.rs::run_spec_benchmark_inner` almost exactly:
1. `unload_model_before_benchmark()` — unload any active server for this model
2. Load config, open DB, resolve model config
3. `resolve_model_path()` — resolve to GGUF file path (import from `run` module)
4. Build `MtpBenchConfig`
5. Create `MtpBenchProgressSink` (same pattern as spec.rs — implements `ProgressSink`, logs to job, broadcasts result)
6. Resolve backend path, discover llama-server binary
7. Call `tama_core::bench::llama_cli_mtp::run_mtp_bench(&mtp_config, Some(server_binary), sink.clone())`
8. Store results in DB via `insert_benchmark` with `engine: "llama_cli_mtp"`
9. Call `sink.result()` with serialized JSON

**Progress sink:** Copy the `SpecBenchProgressSink` pattern from `spec.rs`, rename to `MtpBenchProgressSink`. Same implementation — `log()` appends to job, `result()` broadcasts and stores.

2. Modify `crates/tama-web/src/api/benchmarks/mod.rs`:
- Add `mod mtp;`
- Add `pub use mtp::run_mtp_benchmark;` to re-exports

3. Modify `crates/tama-web/src/server.rs`:
- Register route: `post("/benchmarks/mtp-run", benchmarks::mtp::run_mtp_benchmark)` under the `/tama/v1` prefix (same nesting as `/benchmarks/spec-run`)

**Steps:**
- [ ] Create `crates/tama-web/src/api/benchmarks/mtp.rs` with request DTO, handler, and inner function
- [ ] Add `mod mtp;` and re-export to `crates/tama-web/src/api/benchmarks/mod.rs`
- [ ] Register route in `crates/tama-web/src/server.rs`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Commit with message: "feat: add MTP benchmark API endpoint"

**Acceptance criteria:**
- [ ] `POST /tama/v1/benchmarks/mtp-run` accepts `MtpBenchmarkRunRequest` and returns `{ job_id }`
- [ ] Background job runs `run_mtp_bench` and stores results in DB with `engine: "llama_cli_mtp"`
- [ ] SSE progress streaming works (same pattern as spec benchmark)
- [ ] No clippy warnings

---

### Task 4: Create MTP Testing web UI tab

**Context:**
The Benchmarks page currently has two tabs: "LLaMA-Bench" and "Spec Decoding". We need a third tab "MTP Testing" with a form to configure and run MTP benchmarks, plus a results display. The form mirrors the spec-decode tab's pattern (model selector, backend selector, config fields) but with MTP-specific fields (draft-n-max range, GPU layers).

**Files:**
- Create: `crates/tama-web/src/pages/benchmarks/mtp_bench.rs`
- Modify: `crates/tama-web/src/pages/benchmarks/mod.rs` (add mtp_bench module, add tab button, add conditional rendering)

**What to implement:**

1. Create `crates/tama-web/src/pages/benchmarks/mtp_bench.rs`:

**Component:** `pub fn MtpBench() -> impl IntoView`

**State signals:**
- `selected_display_name`, `selected_model`, `available_models` — model selection (same pattern as spec_bench.rs)
- `selected_backend`, `available_backends` — backend selection (same pattern)
- `draft_max_str` — RwSignal<String>, default "0,1,2,3,4,5,6,7,8"
- `ngl` — RwSignal<String>, default "99"
- `flash_attn` — RwSignal<bool>, default true
- `is_running`, `current_job_id`, `benchmark_results`, `error_msg` — job state

**Form sections (each in `<section class="card">`):**
- **Model**: Two-column grid with Model dropdown and Quant dropdown (exact same as spec_bench.rs)
- **Backend**: Backend dropdown (exact same as spec_bench.rs)
- **MTP Configuration**: 
  - Draft-n-max values: text input, default "0,1,2,3,4,5,6,7,8", hint "Comma-separated, e.g. 0,1,2,3,4,5,6,7,8"
  - GPU layers: text input, default "99", hint "GPU layers for the draft model (default 99)"
  - Flash attention: checkbox, default on
- **Run button**: "▶ Run MTP Benchmark" (disabled when no model selected or running)

**Submit handler:**
- Parse model_id:quant, backend name:variant
- Parse draft_max values via `parse_sizes`
- POST to `/tama/v1/benchmarks/mtp-run` with JSON body
- On success, set `current_job_id`

**SSE callbacks:** Follow the `spec_bench.rs` pattern exactly:
- `on_result_cb`: stores results in `benchmark_results` signal AND sets `is_running.set(false)` (the result event is the reliable signal that work is done)
- `on_status_cb`: also sets `is_running.set(false)` as a fallback when status != "running"

**Results display:**
When `benchmark_results` has data, render:

For each distinct draft_max value (extracted from `entry.draft_max` field), show a group:
- Section header: `h4` with "Draft-n-max: N"
- Table with columns: `Prompt | Wall (s) | Pred | Draft | Acc | Rate | tok/s`
- Rows for each of the 9 prompts
- Aggregate row at bottom of each group: `Wall total`, `Total pred`, `Total draft`, `Total acc`, `Accept rate`, `Avg tok/s`

Entries are already tagged with `draft_max` (from Task 2's `MtpPromptResult`), so grouping is straightforward — collect entries by `draft_max` value, sort by draft_max ascending.

2. Modify `crates/tama-web/src/pages/benchmarks/mod.rs`:
- Add `mod mtp_bench;` at top
- Add `use self::mtp_bench::MtpBench;`
- Add third tab button:
```rust
<button class=move || if active_tab.get() == "mtp-testing" { "btn btn-sm btn-primary" } else { "btn btn-sm btn-outline-secondary" }
        on:click=move |_| active_tab.set("mtp-testing")>
    "MTP Testing"
</button>
```
- In the tab conditional rendering, add MTP case:
```rust
if active_tab.get() == "mtp-testing" {
    view! { <MtpBench /> }.into_any()
} else if active_tab.get() == "spec-decode" {
    // existing spec-decode rendering
} else {
    // existing llama-bench rendering
}
```

**Steps:**
- [ ] Create `crates/tama-web/src/pages/benchmarks/mtp_bench.rs` with full component
- [ ] Add `mod mtp_bench;` and import to `crates/tama-web/src/pages/benchmarks/mod.rs`
- [ ] Add third tab button and conditional rendering in `mod.rs`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Commit with message: "feat: add MTP Testing tab to Benchmarks page"

**Acceptance criteria:**
- [ ] Third tab "MTP Testing" appears alongside "LLaMA-Bench" and "Spec Decoding"
- [ ] Form has model selector, backend selector, draft-n-max input, GPU layers input, flash attention checkbox
- [ ] Submit sends POST to `/tama/v1/benchmarks/mtp-run`
- [ ] Results display groups entries by draft_max value with per-prompt tables
- [ ] JobLogPanel shows progress during benchmark execution
- [ ] No clippy warnings

---

### Task 5: Build verification and history integration

**Context:**
Final verification that the full workspace builds cleanly. Also ensure MTP benchmark results appear correctly in the benchmark history table with appropriate badge styling.

**Files:**
- Modify: `crates/tama-web/src/pages/benchmarks/mod.rs` (history badge for "mtp_sweep" type)
- Modify: `crates/tama-web/src/pages/benchmarks/types.rs` (add "mtp_sweep" to `BENCHMARK_TYPES` if needed)

**What to implement:**

1. Add "mtp_sweep" to `BENCHMARK_TYPES` in `types.rs`:
```rust
pub const BENCHMARK_TYPES: &[(&str, &str)] = &[
    // ... existing entries ...
    ("mtp_sweep", "MTP Sweep"),
];
```

2. In the history table rendering in `mod.rs`, add a badge class for `mtp_sweep`:
```rust
Some("mtp_sweep") => "badge badge-info",  // distinct from muted (llama-bench) and danger (spec-decode)
```
(Uses `badge-info` to visually distinguish MTP from both llama-bench (`badge-muted`) and spec-decode (`badge-danger`)).

3. Full workspace verification:
- `cargo build --workspace`
- `cargo fmt --all`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

**Steps:**
- [ ] Add "mtp_sweep" to `BENCHMARK_TYPES` in `types.rs`
- [ ] Add mtp_sweep badge handling in history table rendering
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix errors.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures.
- [ ] Commit with message: "chore: integrate MTP benchmark into history and verify workspace"

**Acceptance criteria:**
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] MTP benchmark results show in history with "mtp_sweep" badge
- [ ] No formatting issues

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Add `chat_complete` to `ServerHandle` | `llama_cli_spec/server.rs` |
| 2 | Create `llama_cli_mtp` core module | New `llama_cli_mtp/mod.rs`, `bench/mod.rs` |
| 3 | Create MTP API endpoint | New `api/benchmarks/mtp.rs`, `mod.rs`, `server.rs` |
| 4 | Create MTP Testing web UI tab | New `mtp_bench.rs`, `benchmarks/mod.rs` |
| 5 | Build verification + history integration | `types.rs`, `mod.rs` |

**Dependencies:** Task 1 → Task 2 → Task 3 → Task 4 → Task 5 (sequential, each builds on the previous)

**Estimated effort:** ~4-6 hours total
