# Speculative Decoding Benchmark Plan

**Goal:** Add a benchmark mode that uses `llama-cli` to test speculative decoding configurations and find optimal settings for a given model/hardware.

**Architecture:** New `bench/llama_cli_spec/` module in tama-core discovers and invokes `llama-cli` with spec-decoding flags, parses timing output, and returns structured results. New API endpoint (`POST /tama/v1/benchmarks/spec-run`) follows the existing job/SSE pattern. Frontend adds a "Spec Decoding" section on the `/benchmarks` page with form inputs, preset buttons, and a results table sorted by delta vs baseline.

**Tech Stack:** Rust (tama-core, tama-web), Leptos (frontend), llama-cli binary (from llama.cpp backend)

---

### Task 1: Backend module — `bench/llama_cli_spec` (tama-core)

**Context:**
`llama-bench` does not support speculative decoding flags. We need a new benchmark mode that invokes `llama-cli` instead, which supports `--spec-type`, `--draft-max`, and related flags. This task creates the pure-library module that handles binary discovery, argument construction, execution, and output parsing. It mirrors the existing `bench/llama_bench/` structure so the executing agent can follow patterns directly.

**Files:**
- Create: `crates/tama-core/src/bench/llama_cli_spec/mod.rs`
- Create: `crates/tama-core/src/bench/llama_cli_spec/args.rs`
- Create: `crates/tama-core/src/bench/llama_cli_spec/discovery.rs`
- Create: `crates/tama-core/src/bench/llama_cli_spec/parse.rs`
- Modify: `crates/tama-core/src/bench/mod.rs` (add `pub mod llama_cli_spec;`)

**What to implement:**

#### 1a. Types and config (`mod.rs`)

Add these types to `mod.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Speculative decoding type (maps to --spec-type CLI flag).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpecType {
    NgramSimple,
    NgramMod,
    NgramMapK,
    NgramMapK4v,
}

impl SpecType {
    /// Returns the CLI flag value for --spec-type.
    pub fn as_str(&self) -> &'static str {
        match self {
            SpecType::NgramSimple => "ngram-simple",
            SpecType::NgramMod => "ngram-mod",
            SpecType::NgramMapK => "ngram-map-k",
            SpecType::NgramMapK4v => "ngram-map-k4v",
        }
    }
}

/// Configuration for a speculative decoding benchmark sweep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecBenchConfig {
    /// Paths to the target model GGUF file.
    pub model_path: std::path::PathBuf,
    /// Spec types to test (e.g. [NgramSimple, NgramMod]).
    pub spec_types: Vec<SpecType>,
    /// Draft max values to sweep (e.g. [8, 16, 32, 64]).
    pub draft_max_values: Vec<u32>,
    /// N-gram lookup size N values for ngram-mod and ngram-map-* types.
    pub ngram_n_values: Vec<u32>,
    /// N-gram draft size M values for ngram-map-* types.
    pub ngram_m_values: Vec<u32>,
    /// Minimum hits for ngram-map-* types (default 1).
    #[serde(default = "default_min_hits")]
    pub ngram_min_hits: u32,
    /// Number of tokens to generate (-n flag). Default 256.
    #[serde(default = "default_gen_tokens")]
    pub gen_tokens: u32,
    /// Number of repetitions per config. Default 3.
    #[serde(default = "default_runs")]
    pub runs: u32,
    /// GPU layers (maps to --n-gpu-layers). None = use model default.
    pub ngl: Option<u32>,
    /// Flash attention toggle (maps to -fa 1|0). Default true.
    #[serde(default = "default_flash_attn")]
    pub flash_attn: bool,
}

fn default_min_hits() -> u32 { 1 }
fn default_gen_tokens() -> u32 { 256 }
fn default_runs() -> u32 { 3 }
fn default_flash_attn() -> bool { true }
```

Result types:

```rust
/// Result of a single spec-decoding config test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecEntry {
    pub spec_type: String,
    pub draft_max: u32,
    /// N-gram lookup size (only for ngram-mod and ngram-map-*). None for ngram-simple.
    pub ngram_n: Option<u32>,
    /// N-gram draft size (only for ngram-map-*). None for others.
    pub ngram_m: Option<u32>,
    /// Mean token generation speed (tokens/s).
    pub tg_ts_mean: f64,
    /// Stddev of token generation speed.
    pub tg_ts_stddev: f64,
    /// Percentage delta vs baseline. Positive = faster, negative = slower.
    /// Formula: ((tg_ts_mean - baseline_tg_ts) / baseline_tg_ts) * 100
    pub delta_pct: f64,
    /// Status: "success", "failed", or "skipped_oom".
    pub status: String,
    /// Error message if failed. None on success.
    pub error: Option<String>,
}

/// Complete spec benchmark result with baseline and all config entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecBenchResult {
    /// Baseline TG t/s (no spec-decoding) — mean of N runs.
    pub baseline_tg_ts: f64,
    /// Baseline TG t/s stddev.
    pub baseline_tg_stddev: f64,
    /// One entry per config tested.
    pub entries: Vec<SpecEntry>,
}
```

#### 1z. Sweep matrix validation

Before building the sweep, validate that required dimensions are populated for selected spec-types:

```rust
// In run_spec_bench, before sweep generation:
let needs_n = spec_types.iter().any(|t| matches!(t, SpecType::NgramMod | SpecType::NgramMapK | SpecType::NgramMapK4v));
let needs_m = spec_types.iter().any(|t| matches!(t, SpecType::NgramMapK | SpecType::NgramMapK4v));

if needs_n && config.ngram_n_values.is_empty() {
    bail!("ngram_n_values is required when testing ngram-mod or ngram-map-* types");
}
if needs_m && config.ngram_m_values.is_empty() {
    bail!("ngram_m_values is required when testing ngram-map-k or ngram-map-k4v types");
}
```

Sweep matrix dimensions per spec-type:

| spec_type   | dims varied                          |
|-------------|--------------------------------------|
| ngram-simple| spec_type × draft_max                |
| ngram-mod   | spec_type × draft_max × ngram_n      |
| ngram-map-k | spec_type × draft_max × ngram_n × ngram_m |
| ngram-map-k4v| spec_type × draft_max × ngram_n × ngram_m |

**Warning:** With 4 spec-types × 5 draft_max × 4 n × 3 m × 3 runs, that's up to 2160 llama-cli invocations. The UI should show an estimated runtime hint.

#### 1b. Argument construction (`args.rs`)

Implement `build_args(config: &SpecBenchConfig, spec_type: SpecType, draft_max: u32, ngram_n: Option<u32>, ngram_m: Option<u32>) -> Vec<String>`.

Rules — each spec-type only gets the knobs it uses:
- **ngram-simple**: `--spec-type ngram-simple --draft-max N`
- **ngram-mod**: `--spec-type ngram-mod --spec-ngram-size-n N --draft-min M --draft-max MAX` (where M = draft_max / 2, clamped to ≥ 1)
- **ngram-map-k**: `--spec-type ngram-map-k --spec-ngram-size-n N --spec-ngram-size-m M --draft-max MAX`
- **ngram-map-k4v**: same as ngram-map-k but with `--spec-type ngram-map-k4v`

Always include: `-m <model_path>`, `-n <gen_tokens>`, `-ngl <ngl>` (if Some), `-fa 1|0`, `-no-cnv`, `-sp 0` (suppress prompt output), `-ojson` (not needed — we parse stderr timing).

For the **baseline** run: omit all `--spec-*` and `--draft-*` flags entirely.

Also implement `build_baseline_args(config: &SpecBenchConfig) -> Vec<String>` that builds args without any spec flags.

The prompt: use `crate::bench::build_prompt(512)` as the `-p` argument (the existing function that generates ~512 token text).

**Tests:** Write unit tests for each spec-type verifying correct flag emission. Test that baseline args omit all spec flags. Test that ngram-mod computes draft-min correctly.

#### 1c. Binary discovery (`discovery.rs`)

Implement `find_llama_cli(backend_path: &std::path::Path) -> anyhow::Result<std::path::PathBuf>`.

Search order (aligned with existing `llama_bench/discovery.rs` pattern):
1. `LLAMA_CLI_PATH` env var
2. `<backend_path>/bin/llama-cli` (or `llama-cli.exe` on Windows)
3. `<backend_path>/build/bin/llama-cli`
4. `<backend_path>/bin/release/llama-cli`
5. Grandparent-relative: `<backend_path>/../tools/llama-cli/llama-cli`
6. `PATH` lookup for `llama-cli`

Return `anyhow::bail!` with a clear message listing searched paths if not found.

**Tests:** Write unit tests that create temp directories with mock binaries and verify discovery order.

#### 1d. Output parsing (`parse.rs`)

Implement `parse_timing(output: &str) -> anyhow::Result<f64>`.

llama-cli outputs this line to stderr on completion:
```
   total eval time = 2345.67 ms / 256 tokens ( 9.16 ms per token, 109.14 tokens per second)
```

Use regex: `r#"total eval time = [\d.]+ ms / \d+ tokens \([\d.]+ ms per token, ([\d.]+) tokens per second\)"#`

Extract the first capture group (tokens per second). Return as f64.

On no match, return error with context:
```rust
anyhow::bail!("No timing line found in output. Expected: 'total eval time = ...'. Raw output:\n{}", output)
```

The orchestrator should try stderr first, then fallback to stdout if no timing line found (some llama-cli versions write timing to stdout).

**Tests:** Write unit tests with:
- Normal output string → correct t/s extracted
- Different numbers → correct t/s extracted
- Malformed/no match → error returned (verify error message contains "No timing line")

#### 1e. Orchestrator (`mod.rs`)

Implement `pub async fn run_spec_bench(config: &SpecBenchConfig, progress: &dyn ProgressSink) -> anyhow::Result<SpecBenchResult>`.

Algorithm:
1. Discover llama-cli binary via `discovery::find_llama_cli()`
2. Run baseline: `build_baseline_args()` → execute N times → compute mean ± stddev of t/s
3. Build sweep matrix: for each spec_type × draft_max_value, generate one config entry. For ngram-mod/ngram-map-*, also cross with ngram_n_values (and ngram_m_values for map-* types).
4. For each config: execute N times, compute mean ± stddev, compute delta_pct vs baseline
5. After each run, call `progress.log(&format!("[{}/{}] {} d={} → {:.1} t/s ({:+.1}%)", index, total, spec_type, draft_max, tg_ts, delta))`
6. Return `SpecBenchResult { baseline_tg_ts, baseline_tg_stddev, entries }`

Error handling:
- On CLI failure (non-zero exit): retry once (2 total attempts). If still fails, mark entry as `status: "failed"` with stderr snippet in `error` field. Continue sweep.
- On parse failure: same — mark as failed with raw line.
- On OOM detected in stderr (contains "oom" or "out of memory"): halt phase, mark remaining as `status: "skipped_oom"`.

Use `tokio::process::Command` with `stdout(Stdio::piped())` and `stderr(Stdio::piped())`. Parse stderr for timing line.

**Steps:**
- [ ] Write failing test for `parse_timing` with sample output in `parse.rs`
  - `cargo test --package tama-core llama_cli_spec::parse::tests`
  - Did it fail? If not, investigate.
- [ ] Write failing test for `parse_timing` with malformed output (should return error with "No timing line" message)
- [ ] Implement `parse_timing` in `parse.rs` with regex + error context
- [ ] Run `cargo test --package tama-core llama_cli_spec::parse::tests` — verify pass
- [ ] Write failing test for `build_args` with each spec-type in `args.rs`
  - `cargo test --package tama-core llama_cli_spec::args::tests`
- [ ] Write failing test: `build_baseline_args()` returns args WITHOUT `--spec-*` or `--draft-*` flags (verify each flag is absent)
- [ ] Implement `build_args` and `build_baseline_args` in `args.rs`
- [ ] Run tests — verify pass
- [ ] Write failing test for `find_llama_cli` in `discovery.rs` (temp dir with mock binary)
- [ ] Implement `find_llama_cli` in `discovery.rs` with env var + path fallback
- [ ] Run tests — verify pass
- [ ] Write failing test for `run_spec_bench` with a mock binary that fails (use `std::env::current_exe()` with `--version` flag, verify early bail-out)
- [ ] Write test for sweep matrix: given 2 spec-types × 3 draft_max values, verify correct entry count per type
- [ ] Implement `run_spec_bench` orchestrator in `mod.rs` with:
  - Baseline run (N runs → mean ± stddev)
  - Sweep generation with per-type dimension filtering
  - Retry logic (2 attempts total on CLI failure)
  - OOM detection (stderr contains "oom" or "out of memory" → halt, mark remaining `skipped_oom`)
  - stderr-first + stdout fallback for timing parsing
- [ ] Add `pub mod llama_cli_spec;` to `crates/tama-core/src/bench/mod.rs`
- [ ] Run `cargo build --package tama-core` — verify compiles
- [ ] Run `cargo test --package tama-core llama_cli_spec` — verify all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings` — fix any warnings
- [ ] Commit with message: "feat: add llama_cli_spec benchmark module for speculative decoding"

**Acceptance criteria:**
- [ ] Module compiles without warnings (`cargo clippy -D warnings`)
- [ ] All unit tests pass (args, parse, discovery)
- [ ] `build_args` correctly emits flags for all 4 spec-types and baseline
- [ ] `parse_timing` extracts t/s from sample llama-cli output
- [ ] `find_llama_cli` finds binary in standard paths

---

### Task 2: API endpoint — `POST /tama/v1/benchmarks/spec-run` (tama-web)

**Context:**
The existing benchmark API (`/tama/v1/benchmarks/run`) uses a job/SSE pattern: submit → get job_id → connect SSE for progress/results. This task adds a parallel endpoint for spec benchmarks using the same pattern. Results are stored in the existing `benchmarks` DB table with `engine: "llama_cli_spec"`.

**Files:**
- Modify: `crates/tama-web/src/api/benchmarks.rs`
- Modify: `crates/tama-web/src/server.rs` (add route)

**What to implement:**

#### 2a. Request/Response DTOs

Add to `api/benchmarks.rs`:

```rust
use tama_core::bench::llama_cli_spec::{SpecBenchConfig, SpecType};

#[derive(Debug, Clone, Deserialize)]
pub struct SpecBenchmarkRunRequest {
    pub model_id: String,
    #[serde(default)]
    pub backend_name: Option<String>,
    pub spec_types: Vec<SpecType>,
    #[serde(default)]
    pub draft_max_values: Vec<u32>,
    #[serde(default)]
    pub ngram_n_values: Vec<u32>,
    #[serde(default)]
    pub ngram_m_values: Vec<u32>,
    #[serde(default)]
    pub ngram_min_hits: u32,
    #[serde(default)]
    pub gen_tokens: u32,
    #[serde(default)]
    pub runs: u32,
    #[serde(default)]
    pub ngl: Option<u32>,
    #[serde(default)]
    pub flash_attn: bool,
}
```

Apply defaults: use `SpecBenchConfig` serde defaults (single source of truth). Only guard against zero values:
- `runs: req.runs.max(1)` — must be at least 1
- `gen_tokens: req.gen_tokens.max(1)` — must be at least 1
- Empty `spec_types` → return 400 Bad Request
- After building config, call validation: if sweep would produce zero entries (e.g., ngram-mod selected but ngram_n_values empty), return 400 with descriptive error.

#### 2b. Handler

Add `pub async fn run_spec_benchmark(State(state): State<Arc<AppState>>, Json(req): Json<SpecBenchmarkRunRequest>) -> impl IntoResponse`.

Follow the exact same pattern as `run_benchmark()`:
1. Submit job via `jobs.submit(JobKind::Benchmark, None)`
2. Spawn background task
3. Return `(StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id }))`

Background task:
1. Load config, resolve model path (same as existing benchmark handler — use the same `resolve_model_path` pattern from `llama_bench::run_llama_bench`)
2. Build `SpecBenchConfig` from request + resolved model path
3. Create `BenchProgressSink` (reuse the same sink adapter struct from existing benchmark handler)
4. Call `llama_cli_spec::run_spec_bench(&spec_config, &sink)`
5. On success: serialize `SpecBenchResult` to JSON, store in DB via `insert_benchmark` with:
   - `engine: "llama_cli_spec"`
   - `results_json`: full SpecBenchResult JSON
   - `pp_sizes_json: "[]"`, `tg_sizes_json`: `"[gen_tokens]"`
   - `status: "success"` (or `"cancelled"` if job was cancelled)
6. On failure: `jobs.finish(&job, JobStatus::Failed, Some(error))`

#### 2c. Route

Add to `server.rs` alongside existing benchmark routes:
```rust
.broute("/benchmarks/spec-run", post(benchmarks::run_spec_benchmark))
```

**Steps:**
- [ ] Add `SpecBenchmarkRunRequest` DTO to `api/benchmarks.rs`
- [ ] Implement `run_spec_benchmark` handler following the existing `run_benchmark` pattern
  - Copy the job submission, progress sink, and DB storage pattern
  - Use `llama_cli_spec::run_spec_bench` instead of `llama_bench::run_llama_bench`
  - Store with `engine: "llama_cli_spec"`
- [ ] Add route in `server.rs`: `.broute("/benchmarks/spec-run", post(benchmarks::run_spec_benchmark))`
- [ ] Run `cargo build --package tama-web` — verify compiles
- [ ] Run `cargo clippy --package tama-web -- -D warnings` — fix any warnings
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add spec-decoding benchmark API endpoint"

**Acceptance criteria:**
- [ ] Endpoint accepts POST with SpecBenchmarkRunRequest body
- [ ] Returns 202 with job_id on success
- [ ] Returns 409 if another job is already running
- [ ] Background task runs spec bench, streams progress via SSE, stores results in DB
- [ ] Results stored with `engine: "llama_cli_spec"`

---

### Task 3: Frontend UI — Spec Decoding section (tama-web)

**Context:**
The `/benchmarks` page currently only has the llama-bench form. This task adds a "Spec Decoding" tab/section that lets users configure and run spec-decoding benchmarks. The existing page uses Leptos signals and SSE via `JobLogPanel`.

**Files:**
- Modify: `crates/tama-web/src/pages/benchmarks/mod.rs`
- Create: `crates/tama-web/src/pages/benchmarks/spec_bench.rs`

**What to implement:**

#### 3a. Extract spec bench into separate component

Create `spec_bench.rs` with a `#[component] pub fn SpecBench()` component. Keep the existing llama-bench UI in `mod.rs`. Add a tab toggle at the top of the page.

Tab structure — add near the top of the `Benchmarks()` component (before the model selection section):

```rust
let active_tab = RwSignal::new("llama-bench"); // or "spec-decode"

// Tab buttons
view! {
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
}
```

When `active_tab == "spec-decode"`, render `<SpecBench />` instead of the existing llama-bench form.

#### 3b. Spec bench form signals

In `spec_bench.rs`, create these signals:
- `selected_model: RwSignal<String>` — model id (reuse same model fetch as main page)
- `spec_types: RwSignal<Vec<SpecType>>` — checked types (default: `[NgramSimple, NgramMod]`)
- `draft_max_str: RwSignal<String>` — comma-separated (default: "8,16,32,64")
- `ngram_n_str: RwSignal<String>` — comma-separated (default: "12,16,24")
- `ngram_m_str: RwSignal<String>` — comma-separated (default: "32,48")
- `gen_tokens: RwSignal<u32>` — default 256
- `runs: RwSignal<u32>` — default 3
- `is_running: RwSignal<bool>`
- `current_job_id: RwSignal<Option<String>>`
- `benchmark_results: RwSignal<Option<serde_json::Value>>`

#### 3c. Form UI

Build the form with these sections:

**Model selection:** Same dropdown as existing page (fetch `/tama/v1/models`, show display_name + quant).

**Backend selection:** Same dropdown as existing llama-bench page (fetch `/tama/v1/backends`). "Auto (model's backend)" as default. Pass `backend_name` in the request DTO.

**Spec types to test:** Checkboxes for each type:
```
☑ ngram-simple — Simple n-gram pattern matching
☑ ngram-mod — Rolling hash pool (best for code/reasoning)
☐ ngram-map-k — Hash map with key tracking
☐ ngram-map-k4v — Key + 4 values (experimental, long repetitions)
```

**Knob fields:** Comma-separated text inputs:
- "Draft max values" — default "8,16,32,64", hint "Tokens to draft per round"
- "N-gram size N" — default "12,16,24", hint "Lookup pattern length (for ngram-mod/map)"
- "N-gram size M" — default "32,48", hint "Draft pattern length (for ngram-map-k/k4v only)"

**Run settings:**
- "Generation tokens" — number input, default 256
- "Runs per config" — number input, default 3

#### 3d. Preset buttons

Add 4 preset buttons that auto-fill the form:
1. **"Quick filter"** — spec_types: all 4, draft_max: "16", n: "12", m: "48", gen: 256, runs: 3
2. **"Draft sweep"** — spec_types: [ngram-simple, ngram-mod], draft_max: "8,16,32,48,64", n: "12", m: "48", gen: 256, runs: 3
3. **"N-gram sweep"** — spec_types: [ngram-mod], draft_max: "32", n: "8,12,16,24", m: "32,48,64", gen: 256, runs: 3
4. **"Depth test"** — (same as N-gram sweep but adds a hint to manually set depth in advanced settings)

#### 3e. Submit handler

On "▶ Run Spec Benchmark" click:
1. Parse form fields into `SpecBenchmarkRunRequest`
2. POST to `/tama/v1/benchmarks/spec-run`
3. On success: set `current_job_id`, show `JobLogPanel` (reuse existing component)
4. SSE callbacks: `on_result` stores parsed JSON, `on_status` clears `is_running` when done

#### 3f. Results display

When `benchmark_results` has data, render a results table:

```
| Spec Type | Draft Max | N | M | t/s (± stddev) | Δ vs baseline |
```

- First row is always the baseline: spec_type = "— (baseline)", other columns = "—"
- Sort remaining rows by `delta_pct` descending (best first)
- `delta_pct > 0`: green badge (`badge badge-success`)
- `delta_pct < 0`: red badge (`badge badge-danger`)
- `delta_pct == 0`: muted badge

Format delta as `+14.7%` or `−3.2%` (use Unicode minus for negative).

Badge threshold (floating-point safe):
```rust
if delta_pct > 0.5 { "badge badge-success" }    // green
else if delta_pct < -0.5 { "badge badge-danger" } // red
else { "badge badge-muted" }                     // ~equal, gray
```

**Steps:**
- [ ] Create `spec_bench.rs` with empty `SpecBench` component
- [ ] Add tab toggle to `mod.rs` Benchmarks component
- [ ] Implement form signals and model selection in `spec_bench.rs`
- [ ] Build spec type checkboxes UI
- [ ] Build knob input fields with defaults
- [ ] Build preset buttons that auto-fill form
- [ ] Implement submit handler with POST to `/tama/v1/benchmarks/spec-run`
- [ ] Wire up JobLogPanel for SSE progress (reuse existing component)
- [ ] Implement results table with baseline row, sorted entries, colored delta badges
- [ ] Run `cargo build --package tama-web` — verify compiles
- [ ] Run `cargo clippy --package tama-web -- -D warnings` — fix warnings
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add spec-decoding benchmark UI with presets and results table"

**Acceptance criteria:**
- [ ] Tab toggle switches between llama-bench and spec-decoding views
- [ ] Form has all inputs: model selector, spec-type checkboxes, knob fields, run settings
- [ ] Preset buttons auto-fill form fields correctly
- [ ] Submit sends correct JSON to `/tama/v1/benchmarks/spec-run`
- [ ] JobLogPanel shows live progress during benchmark execution
- [ ] Results table displays baseline + entries sorted by delta
- [ ] Delta badges are green for positive, red for negative

---

## Notes

- **DB storage:** No migration needed. Results stored in existing `benchmarks` table with `engine: "llama_cli_spec"`. The `results_json` column holds the full `SpecBenchResult` JSON. History page shows these entries alongside llama-bench results.
- **Job cancellation:** User can cancel via UI (close/disconnect SSE). Partial results are saved with `status: "cancelled"`. No resume support in v1.
- **Baseline:** Runs once per job (not re-run each phase). All entries compare against that single baseline mean.
- **ngram-min-hits:** Default 1. Only exposed as an advanced field if user needs it — not in the default form.
- **Flash attention:** Enabled by default (`true`), passed as `-fa 1`. Compatible with all spec-decoding types.
