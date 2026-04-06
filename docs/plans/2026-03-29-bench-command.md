# `koji bench` — HTTP API Benchmark Command

**Goal:** Add a `koji bench <name>` CLI command that spins up a llama-server backend, sends OpenAI-compatible streaming chat requests, measures inference timing from the SSE stream, and prints results as a formatted terminal table.
**Status:** ✅ COMPLETED - See git commit `4bf65f7` ("feat: add kronk bench command for LLM inference benchmarking (#22)")

**Architecture:** The feature adds a `bench` module to `koji-core` containing pure-logic types/statistics, an HTTP streaming measurement client, and a backend process runner/orchestrator. The CLI gets a new `Bench` command variant that wires to a handler calling the core module. The bench runner reuses existing infrastructure: `Config::resolve_server()` and `Config::build_full_args()` for argument resolution, `check_health()` for readiness polling, and `kill_process()` / `force_kill_process()` for teardown.

**Tech Stack:** Rust, tokio (async runtime + process), reqwest (HTTP + SSE streaming), serde_json (SSE chunk parsing), clap (CLI args). All dependencies already exist in the workspace — zero new crates needed.

---

### Task 1: Core bench types, statistics, prompt builder, and display formatting

**Context:**
This task creates the `bench` module in `koji-core` with all the pure-logic building blocks: data types for configuration and results, statistical aggregation (mean/stddev), a deterministic prompt builder that produces prompts of approximate token counts, and a terminal table formatter. All of this is pure logic with no IO, making it heavily unit-testable. This task establishes the data model that all subsequent tasks build on. The bench feature measures 4 metrics: prompt processing speed (PP t/s), token generation speed (TG t/s), time to first token (TTFT), and total request latency. PP speed is derived as `prompt_tokens / ttft_seconds`, TG speed as `(generated_tokens - 1) / (total_seconds - ttft_seconds)`.

**Files:**
- Create: `crates/koji-core/src/bench/mod.rs`
- Create: `crates/koji-core/src/bench/display.rs`
- Modify: `crates/koji-core/src/lib.rs`

**What to implement:**

**In `crates/koji-core/src/bench/mod.rs`:**

1. Add module declarations at the top:
   ```rust
   pub mod display;
   pub mod measure;   // created in Task 2
   pub mod runner;    // created in Task 3
   ```
   For now, create empty placeholder files `crates/koji-core/src/bench/measure.rs` and `crates/koji-core/src/bench/runner.rs` containing only `//! Placeholder — implemented in a later task.` so the module compiles.

2. Define `BenchConfig` struct:
   ```rust
   #[derive(Debug, Clone)]
   pub struct BenchConfig {
       pub pp_sizes: Vec<u32>,       // prompt token counts to test (default: [512])
       pub tg_sizes: Vec<u32>,       // generation lengths to test (default: [128])
       pub runs: u32,                // measurement iterations (default: 3)
       pub warmup: u32,              // warmup iterations (default: 1)
       pub ctx_override: Option<u32>, // optional context size override
   }
   ```
   Implement `Default` for `BenchConfig` with `pp_sizes: vec![512]`, `tg_sizes: vec![128]`, `runs: 3`, `warmup: 1`, `ctx_override: None`.

3. Define `RequestMeasurement` struct:
   ```rust
   #[derive(Debug, Clone)]
   pub struct RequestMeasurement {
       pub prompt_tokens: u32,
       pub generated_tokens: u32,
       pub ttft_ms: f64,           // time to first token in milliseconds
       pub total_ms: f64,          // total request time in milliseconds
       pub pp_tokens_per_sec: f64, // prompt_tokens / (ttft_ms / 1000)
       pub tg_tokens_per_sec: f64, // (generated_tokens - 1) / ((total_ms - ttft_ms) / 1000)
   }
   ```

4. Define `BenchSummary` struct:
   ```rust
   #[derive(Debug, Clone)]
   pub struct BenchSummary {
       pub test_name: String,       // e.g. "pp512/tg128"
       pub prompt_tokens: u32,
       pub gen_tokens: u32,
       pub pp_mean: f64,            // mean PP tokens/s
       pub pp_stddev: f64,          // stddev PP tokens/s
       pub tg_mean: f64,            // mean TG tokens/s
       pub tg_stddev: f64,          // stddev TG tokens/s
       pub ttft_mean: f64,          // mean TTFT ms
       pub ttft_stddev: f64,        // stddev TTFT ms
       pub total_mean: f64,         // mean total ms
       pub total_stddev: f64,       // stddev total ms
   }
   ```

5. Define `ModelInfo` struct (metadata for display):
   ```rust
   #[derive(Debug, Clone)]
   pub struct ModelInfo {
       pub name: String,            // server config name
       pub model_id: Option<String>, // e.g. "bartowski/Qwen2.5-Coder-7B-GGUF"
       pub quant: Option<String>,    // e.g. "Q4_K_M"
       pub backend: String,          // backend config name
       pub gpu_type: String,         // e.g. "CUDA", "Vulkan", "CPU"
       pub context_length: Option<u32>,
       pub gpu_layers: Option<String>,
   }
   ```

6. Define `BenchReport` struct:
   ```rust
   #[derive(Debug, Clone)]
   pub struct BenchReport {
       pub model_info: ModelInfo,
       pub config: BenchConfig,
       pub summaries: Vec<BenchSummary>,
       pub load_time_ms: f64,
       pub vram: Option<crate::gpu::VramInfo>,
   }
   ```

7. Implement `compute_summary(test_name: &str, prompt_tokens: u32, gen_tokens: u32, measurements: &[RequestMeasurement]) -> BenchSummary`:
   - Extract `pp_tokens_per_sec` values from all measurements, compute mean and stddev.
   - Same for `tg_tokens_per_sec`, `ttft_ms`, `total_ms`.
   - Return a `BenchSummary` with all computed stats.
   - Mean formula: `sum / count`
   - Stddev formula (population): `sqrt(sum((x - mean)^2) / count)`. Use population stddev (not sample) since we control the number of runs.
   - Handle edge case: if `measurements` is empty, all values are 0.0.
   - Handle edge case: if only 1 measurement, stddev is 0.0.

8. Implement `build_prompt(target_tokens: u32) -> String`:
   - Returns a user message string of approximately `target_tokens` tokens.
   - Strategy: Repeat the sentence `"The quick brown fox jumps over the lazy dog. "` (this is approximately 10 tokens in most LLM tokenizers) until we reach `target_tokens * 4` characters (the ~4 chars/token heuristic for English text with common BPE tokenizers).
   - The function should return just the user content string (not the full messages array — the caller builds that).
   - This is approximate and that's fine — we use it for consistent, reproducible prompts. The actual token count doesn't need to be exact since we're comparing relative performance.

**In `crates/koji-core/src/bench/display.rs`:**

9. Implement `print_bench_report(report: &BenchReport)`:
   - Print a header block:
     ```
     koji bench — <name> (<quant>) via <backend>
     GPU: <gpu_type> | Context: <context_length> | Runs: <runs> | Warmup: <warmup>
     ────────────────────────────────────────────────────────────────────
     ```
     If `quant` is None, omit the parenthetical. If `model_id` is Some, show it after the name. Use `─` (U+2500) for the horizontal line, width 70 chars.
   - Print the results table:
     ```
      Test         │ PP (t/s)        │ TG (t/s)        │ TTFT (ms)  │ Total (ms)
     ─────────────┼─────────────────┼─────────────────┼────────────┼────────────
      pp512/tg128  │ 4821.3 ± 42.1   │ 89.2 ± 1.3      │ 106.2 ± 3  │ 1542.1 ± 28
     ```
     Use `│` (U+2502) for vertical separators, `┼` (U+253C) for crossings.
     Column widths: Test=13, PP=17, TG=17, TTFT=12, Total=12.
     Right-align numeric values within each column.
   - Print a footer:
     ```
     ────────────────────────────────────────────────────────────────────
     Model load time: <load_time_ms> ms
     VRAM: <used_mib> / <total_mib> MiB
     ```
     If `vram` is None, omit the VRAM line.

10. Implement helper `format_stat(mean: f64, stddev: f64) -> String`:
    - If stddev == 0.0, return formatted mean only: e.g. `"4821.3"`.
    - Otherwise return `"4821.3 ± 42.1"`.
    - Use 1 decimal place for all values.

**In `crates/koji-core/src/lib.rs`:**

11. Add `pub mod bench;` after the existing `pub mod backends;` line (keep alphabetical order).

**Unit tests — all in `crates/koji-core/src/bench/mod.rs` in a `#[cfg(test)] mod tests` block:**

- `test_bench_config_default`: Assert `BenchConfig::default()` has pp_sizes=[512], tg_sizes=[128], runs=3, warmup=1, ctx_override=None.
- `test_compute_summary_basic`: Create 3 `RequestMeasurement` values with known numbers, call `compute_summary`, assert mean and stddev are mathematically correct (within f64 epsilon).
- `test_compute_summary_single_measurement`: One measurement → stddev should be 0.0.
- `test_compute_summary_empty`: Empty slice → all values 0.0.
- `test_build_prompt_approximate_length`: Call `build_prompt(512)`, assert the returned string length is between 1500 and 2500 chars (roughly 4 chars/token ± tolerance).
- `test_build_prompt_scales`: Assert `build_prompt(1024).len() > build_prompt(512).len()`.
- `test_format_stat_with_stddev`: Assert `format_stat(4821.3, 42.1)` returns `"4821.3 ± 42.1"`.
- `test_format_stat_zero_stddev`: Assert `format_stat(4821.3, 0.0)` returns `"4821.3"`.

**Steps:**
- [ ] Write all 8 unit tests in `crates/koji-core/src/bench/mod.rs`
- [ ] Run `cargo test --package koji-core --lib bench::tests`, confirm they all fail (module doesn't exist yet)
- [ ] Create `crates/koji-core/src/bench/mod.rs` with all types, `compute_summary`, `build_prompt`, and the test module
- [ ] Create `crates/koji-core/src/bench/display.rs` with `print_bench_report` and `format_stat`
- [ ] Create placeholder files `crates/koji-core/src/bench/measure.rs` and `crates/koji-core/src/bench/runner.rs` with just `//! Placeholder — implemented in a later task.`
- [ ] Add `pub mod bench;` to `crates/koji-core/src/lib.rs`
- [ ] Run `cargo test --package koji-core --lib bench::tests`, confirm all 8 tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`, fix any warnings
- [ ] Run `cargo build --workspace`, confirm it succeeds
- [ ] Commit with message: `feat: add bench module with types, statistics, prompt builder, and display`

**Acceptance criteria:**
- [ ] `crates/koji-core/src/bench/mod.rs` exists with `BenchConfig`, `RequestMeasurement`, `BenchSummary`, `ModelInfo`, `BenchReport` structs
- [ ] `compute_summary` correctly computes mean and stddev from measurements
- [ ] `build_prompt` returns a string that scales proportionally to the requested token count
- [ ] `print_bench_report` outputs a formatted table with Unicode box-drawing characters
- [ ] All 8 unit tests pass
- [ ] `cargo build --workspace` succeeds with no warnings

---

### Task 2: HTTP streaming measurement client

**Context:**
This task implements the HTTP client that sends a single OpenAI-compatible chat completion request to a running llama-server and measures timing from the SSE (Server-Sent Events) stream. This is the core measurement primitive — the orchestrator (Task 3) calls this repeatedly. The function sends a POST to `/v1/chat/completions` with `stream: true`, `temperature: 0`, `max_tokens: <tg_size>`, and a prompt built from `build_prompt()` (from Task 1). It then reads the SSE stream, recording: (a) the time the first `data:` chunk with `delta.content` arrives (TTFT), (b) the total number of content chunks (generated tokens), and (c) the time the stream completes (`data: [DONE]`). It returns a `RequestMeasurement`. The llama-server SSE format is: each line is `data: <json>` where the JSON has `choices[0].delta.content` for token content. The first chunk typically has `delta.role` only (no content) — skip it for TTFT. The stream ends with `data: [DONE]`.

**Files:**
- Modify: `crates/koji-core/src/bench/measure.rs` (replace placeholder)

**What to implement:**

1. Replace the placeholder content in `crates/koji-core/src/bench/measure.rs` with the actual implementation.

2. Implement `pub async fn send_bench_request(base_url: &str, prompt_tokens: u32, max_tokens: u32) -> Result<RequestMeasurement>`:
   - Build the request body as `serde_json::Value`:
     ```json
     {
       "model": "benchmark",
       "messages": [
         {"role": "system", "content": "You are a helpful assistant. Continue generating text without stopping."},
         {"role": "user", "content": "<output of build_prompt(prompt_tokens)>"}
       ],
       "max_tokens": <max_tokens>,
       "temperature": 0,
       "top_p": 1.0,
       "stream": true
     }
     ```
   - Record `request_start = Instant::now()` immediately before sending the HTTP request.
   - Send POST to `{base_url}/v1/chat/completions` using `reqwest::Client` with a 300-second timeout (long timeout since large prompts can take a while to process).
   - The response is an SSE stream. Read it using `response.bytes_stream()` from reqwest (the `stream` feature is already enabled). Collect bytes into a string buffer. Process complete lines (split by `\n`).
   - For each line starting with `data: `:
     - Strip the `data: ` prefix.
     - If the remainder is `[DONE]`, the stream is complete. Record `end_time = Instant::now()`.
     - Otherwise, parse as JSON. Check if `choices[0].delta.content` exists and is a non-empty string.
     - If it is the **first** chunk with content, record `first_token_time = Instant::now()`. This is TTFT.
     - Increment `generated_token_count` for each chunk with non-empty content.
   - After the stream completes, compute and return `RequestMeasurement`:
     - `prompt_tokens`: the input `prompt_tokens` parameter (this is approximate, which is fine for benchmarking)
     - `generated_tokens`: `generated_token_count` (the number of content chunks received)
     - `ttft_ms`: `first_token_time.duration_since(request_start).as_secs_f64() * 1000.0`
     - `total_ms`: `end_time.duration_since(request_start).as_secs_f64() * 1000.0`
     - `pp_tokens_per_sec`: `prompt_tokens as f64 / (ttft_ms / 1000.0)`
     - `tg_tokens_per_sec`: if `generated_tokens > 1` then `(generated_tokens - 1) as f64 / ((total_ms - ttft_ms) / 1000.0)` else `0.0`. We subtract 1 because the first token's generation time is included in TTFT.
   - Error handling: if no content chunks are received, return an error `anyhow::bail!("No tokens generated — the model returned an empty response")`. If `first_token_time` was never set, return an error.

3. Also implement a helper `pub fn parse_sse_content(line: &str) -> Option<String>`:
   - Takes a raw SSE line (e.g., `data: {"id":"...","choices":[{"delta":{"content":"hello"}}]}`)
   - Returns `Some(content_string)` if the line has `data: ` prefix and the JSON contains a non-empty `choices[0].delta.content`.
   - Returns `None` for `data: [DONE]`, lines without `data: ` prefix, chunks with empty/missing content (like the initial `delta.role` chunk), and blank lines.
   - This helper makes the SSE parsing logic testable without needing a real server.

**Unit tests — in `#[cfg(test)] mod tests` block at the bottom of `measure.rs`:**

- `test_parse_sse_content_with_token`: Input `r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#` → returns `Some("hello".to_string())`.
- `test_parse_sse_content_role_chunk`: Input `r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#` → returns `None`.
- `test_parse_sse_content_done`: Input `"data: [DONE]"` → returns `None`.
- `test_parse_sse_content_empty_line`: Input `""` → returns `None`.
- `test_parse_sse_content_empty_content`: Input `r#"data: {"choices":[{"delta":{"content":""}}]}"#` → returns `None`.

**Steps:**
- [ ] Write the 5 unit tests for `parse_sse_content` in `crates/koji-core/src/bench/measure.rs`
- [ ] Run `cargo test --package koji-core --lib bench::measure::tests`, confirm they fail
- [ ] Implement `parse_sse_content` and `send_bench_request` in `crates/koji-core/src/bench/measure.rs`
- [ ] Run `cargo test --package koji-core --lib bench::measure::tests`, confirm all 5 tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`, fix any warnings
- [ ] Run `cargo build --workspace`, confirm it succeeds
- [ ] Commit with message: `feat: add bench HTTP streaming measurement client`

**Acceptance criteria:**
- [ ] `send_bench_request` sends a streaming chat completion request and returns accurate timing in a `RequestMeasurement`
- [ ] `parse_sse_content` correctly extracts token content from SSE lines and returns None for non-content lines
- [ ] All 5 unit tests pass
- [ ] `cargo build --workspace` succeeds with no warnings

---

### Task 3: Backend runner and benchmark orchestrator

**Context:**
This task implements the process lifecycle (start a llama-server, wait for health, run benchmarks, stop the server) and the orchestration logic that ties the types (Task 1) and measurement client (Task 2) together. The runner reuses existing infrastructure from koji-core: `Config::resolve_server()` to look up the server/backend config pair, `Config::build_full_args()` to build the CLI arguments (including model path, context size, GPU layers from model cards), `override_arg()` to set host/port, `check_health()` to poll readiness, and `kill_process()` / `force_kill_process()` for shutdown. The orchestrator starts the backend, runs warmup iterations (results discarded), then runs measured iterations for each (pp_size, tg_size) combination, computes summaries, queries VRAM, stops the backend, and returns a complete `BenchReport`.

**Files:**
- Modify: `crates/koji-core/src/bench/runner.rs` (replace placeholder)

**What to implement:**

1. Replace the placeholder content in `crates/koji-core/src/bench/runner.rs`.

2. Define `BenchBackend` struct (private to this module, not `pub` — it's an internal handle):
   ```rust
   struct BenchBackend {
       pub pid: u32,
       pub url: String,          // e.g. "http://127.0.0.1:54321"
       pub load_time_ms: f64,    // time from spawn to health check passing
   }
   ```

3. Implement `async fn start_backend(config: &Config, server_name: &str, ctx_override: Option<u32>) -> Result<BenchBackend>`:
   - Call `config.resolve_server(server_name)` to get `(server_config, backend_config)`. Propagate error with context `"Failed to resolve server config for bench"`.
   - Record `spawn_start = Instant::now()`.
   - Allocate a free port: `let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?; let port = listener.local_addr()?.port(); drop(listener);`
   - Build args: `let mut args = config.build_full_args(server_config, backend_config, ctx_override)?;`
   - Override host/port: `override_arg(&mut args, "--host", "127.0.0.1"); override_arg(&mut args, "--port", &port.to_string());`
   - Spawn the process: `tokio::process::Command::new(&backend_config.path).args(&args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn()?`. Redirect stdout/stderr to null because we don't need backend logs during benchmarking.
   - Get PID: `child.id().ok_or_else(|| anyhow!("Failed to get backend PID"))?`
   - Spawn a reaper task for the child (same pattern as `crates/koji-core/src/proxy/lifecycle.rs` lines 100-106): `tokio::spawn(async move { let _ = child.wait().await; });`
   - Poll health: loop every 500ms calling `check_health(&format!("http://127.0.0.1:{}/health", port), Some(30)).await`. Timeout after 120 seconds (benchmarking may load large models). If timeout, kill the process and return error.
   - Record `load_time_ms = spawn_start.elapsed().as_secs_f64() * 1000.0`.
   - Return `BenchBackend { pid, url: format!("http://127.0.0.1:{}", port), load_time_ms }`.
   - Use `tracing::info!` for key lifecycle events (starting backend, health check passed, etc.)

4. Implement `async fn stop_backend(backend: &BenchBackend) -> Result<()>`:
   - Import and call `crate::proxy::process::kill_process(backend.pid).await?`.
   - Wait up to 5 seconds polling `is_process_alive(backend.pid)` every 250ms.
   - If still alive after 5s, call `force_kill_process(backend.pid).await?`.
   - Use `tracing::info!` for shutdown events.

5. Implement `pub async fn run_benchmark(config: &Config, server_name: &str, bench_config: &BenchConfig) -> Result<BenchReport>`:
   - Build `ModelInfo` from config data:
     - Call `config.resolve_server(server_name)` to get `(server_config, backend_config)`.
     - `name`: `server_name.to_string()`
     - `model_id`: `server_config.model.clone()`
     - `quant`: `server_config.quant.clone()`
     - `backend`: `server_config.backend.clone()`
     - `gpu_type`: Try to determine from backend config or default to `"Unknown"`. Check if `backend_config.path` contains "vulkan" → "Vulkan", "cuda" → "CUDA", "rocm" → "ROCm", otherwise check for nvidia-smi presence via `crate::gpu::query_vram().is_some()` → "CUDA", else "CPU".
     - `context_length`: `server_config.context_length`
     - `gpu_layers`: scan `server_config.args` for `"-ngl"` flag and take the next value, or None.
   - Print a starting message: `println!("Starting benchmark for '{}'...", server_name);`
   - Start backend: `let backend = start_backend(config, server_name, bench_config.ctx_override).await?;`
   - Print: `println!("Backend loaded in {:.0} ms", backend.load_time_ms);`
   - Initialize `let mut summaries: Vec<BenchSummary> = Vec::new();`
   - For each `pp_size` in `bench_config.pp_sizes`, for each `tg_size` in `bench_config.tg_sizes`:
     - `let test_name = format!("pp{}/tg{}", pp_size, tg_size);`
     - Print: `println!("Running {} (warmup: {}, runs: {})...", test_name, bench_config.warmup, bench_config.runs);`
     - **Warmup phase**: for `_ in 0..bench_config.warmup`, call `send_bench_request(&backend.url, pp_size, tg_size).await?;` and discard the result.
     - **Measurement phase**: for `_ in 0..bench_config.runs`, call `send_bench_request(&backend.url, pp_size, tg_size).await?;` and collect into `Vec<RequestMeasurement>`.
     - Call `compute_summary(&test_name, pp_size, tg_size, &measurements)` and push to `summaries`.
   - Query VRAM: `let vram = crate::gpu::query_vram();`
   - Stop backend: `stop_backend(&backend).await?;`
   - Return `BenchReport { model_info, config: bench_config.clone(), summaries, load_time_ms: backend.load_time_ms, vram }`.
   - If any step after `start_backend` fails, make sure to call `stop_backend` before propagating the error. Use a pattern like:
     ```rust
     let result = run_benchmark_inner(&backend, bench_config).await;
     stop_backend(&backend).await?;
     result
     ```
     Or equivalently, use a helper async closure / function so the backend is always cleaned up.

**Unit tests — in `#[cfg(test)] mod tests` block at the bottom of `runner.rs`:**

These are limited since the runner mostly does IO, but we can test the `ModelInfo` extraction logic:

- `test_gpu_type_from_path_cuda`: A backend path containing "cuda" should produce "CUDA". (Test this by extracting the GPU detection into a small helper `fn detect_gpu_type(backend_path: &str) -> String` and testing that.)
- `test_gpu_type_from_path_vulkan`: Path containing "vulkan" → "Vulkan".
- `test_gpu_type_from_path_default`: Path without known keywords and no nvidia-smi → "CPU".
- `test_extract_gpu_layers_some`: Args `["-m", "model.gguf", "-ngl", "99"]` → `Some("99".to_string())`.
- `test_extract_gpu_layers_none`: Args `["-m", "model.gguf"]` → `None`.

Extract these as small pure helper functions:
- `fn detect_gpu_type(backend_path: &str, has_nvidia: bool) -> String`
- `fn extract_gpu_layers(args: &[String]) -> Option<String>`

**Steps:**
- [ ] Write the 5 unit tests for the helper functions in `crates/koji-core/src/bench/runner.rs`
- [ ] Run `cargo test --package koji-core --lib bench::runner::tests`, confirm they fail
- [ ] Implement `detect_gpu_type`, `extract_gpu_layers`, `start_backend`, `stop_backend`, `run_benchmark` in `crates/koji-core/src/bench/runner.rs`
- [ ] Run `cargo test --package koji-core --lib bench::runner::tests`, confirm all 5 tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`, fix any warnings
- [ ] Run `cargo build --workspace`, confirm it succeeds
- [ ] Commit with message: `feat: add bench backend runner and orchestrator`

**Acceptance criteria:**
- [ ] `start_backend` spawns a llama-server process, waits for health, and returns a `BenchBackend` with url/pid/load_time
- [ ] `stop_backend` gracefully terminates the process with SIGTERM → SIGKILL fallback
- [ ] `run_benchmark` orchestrates the full benchmark: start → warmup → measure → summarize → stop → return report
- [ ] Backend is always cleaned up even if measurement fails
- [ ] All 5 unit tests pass
- [ ] `cargo build --workspace` succeeds with no warnings

---

### Task 4: CLI command, handler, and dispatch

**Context:**
This task wires the benchmarking feature into the koji CLI. It adds a `Bench` variant to the `Commands` enum in `crates/koji-cli/src/cli.rs`, creates a handler function in a new `crates/koji-cli/src/handlers/bench.rs`, registers the handler module, and adds the dispatch arm in `crates/koji-cli/src/lib.rs`. The handler parses comma-separated `--pp` and `--tg` strings into `Vec<u32>`, builds a `BenchConfig`, resolves which model configs to benchmark (single name or `--all` for every enabled config), calls `koji_core::bench::runner::run_benchmark()` for each, and calls `koji_core::bench::display::print_bench_report()` to show results. When `--all` is used, it iterates over all entries in `config.models` where `enabled == true`, running benchmarks sequentially and printing each report.

**Files:**
- Modify: `crates/koji-cli/src/cli.rs`
- Create: `crates/koji-cli/src/handlers/bench.rs`
- Modify: `crates/koji-cli/src/handlers/mod.rs`
- Modify: `crates/koji-cli/src/lib.rs`

**What to implement:**

**In `crates/koji-cli/src/cli.rs`:**

1. Add a new variant to the `Commands` enum. The existing enum is NOT alphabetically ordered (order is: Run, Service, ServiceRun, Add, Update, Server, Status, Profile, Config, Model, Backend, Serve, Proxy, Logs). Add the `Bench` variant **after `Backend`** and **before `Serve`** to keep it grouped with the other user-facing commands:
   ```rust
   /// Benchmark model inference performance
   Bench {
       /// Model config name to benchmark (required unless --all is used)
       name: Option<String>,
       /// Benchmark all enabled model configs sequentially
       #[arg(long)]
       all: bool,
       /// Prompt processing sizes, comma-separated (default: "512")
       #[arg(long, default_value = "512")]
       pp: String,
       /// Token generation lengths, comma-separated (default: "128")
       #[arg(long, default_value = "128")]
       tg: String,
       /// Number of measurement runs per test (default: 3)
       #[arg(long, default_value_t = 3)]
       runs: u32,
       /// Number of warmup runs before measuring (default: 1)
       #[arg(long, default_value_t = 1)]
       warmup: u32,
       /// Override context size (e.g. 4096, 8192)
       #[arg(long)]
       ctx: Option<u32>,
   }
   ```

**In `crates/koji-cli/src/handlers/bench.rs`:**

2. Implement a helper `fn parse_comma_sizes(s: &str) -> Result<Vec<u32>>`:
   - Split `s` by `,`, trim each part, parse as `u32`.
   - Return error if any part fails to parse: `anyhow::bail!("Invalid size '{}': must be a positive integer", part)`.
   - Return error if the result is empty: `anyhow::bail!("At least one size must be specified")`.

3. Implement `pub async fn cmd_bench(config: &Config, name: Option<String>, all: bool, pp: String, tg: String, runs: u32, warmup: u32, ctx: Option<u32>) -> Result<()>`:
   - Parse pp/tg: `let pp_sizes = parse_comma_sizes(&pp)?; let tg_sizes = parse_comma_sizes(&tg)?;`
   - Build `BenchConfig { pp_sizes, tg_sizes, runs, warmup, ctx_override: ctx }`.
   - Determine which servers to benchmark:
     - If `all` is true: collect all server names from `config.models` where `enabled == true`. Sort alphabetically for deterministic order. If none found, `anyhow::bail!("No enabled model configs found. Create one with `koji model create`.")`.
     - If `name` is `Some(n)`: use `vec![n]`. Validate it exists: `config.resolve_server(&n)?;`
     - If `name` is `None` and `all` is `false`: `anyhow::bail!("Specify a model config name or use --all to benchmark all enabled configs")`.
   - For each server name in the list:
     - Call `koji_core::bench::runner::run_benchmark(config, &server_name, &bench_config).await?`.
     - Call `koji_core::bench::display::print_bench_report(&report)`.
     - If there are more servers to benchmark, print a blank line separator.
   - Return `Ok(())`.

**In `crates/koji-cli/src/handlers/mod.rs`:**

4. Add `pub mod bench;` — insert it alphabetically (before `config`).

**In `crates/koji-cli/src/lib.rs`:**

5. Add the dispatch arm in the `match args.command` block. The match arms follow the same order as the enum declaration. Add it **after the `Backend` arm** and **before the `Serve` arm** (look for `Commands::Backend { command } => { ... }` and insert the new arm immediately after it):
   ```rust
   Commands::Bench { name, all, pp, tg, runs, warmup, ctx } => {
       handlers::bench::cmd_bench(&config, name, all, pp, tg, runs, warmup, ctx).await
   }
   ```
   No additional `use` statement needed — just call `handlers::bench::cmd_bench` directly like the other handlers.

**Unit tests — in `#[cfg(test)] mod tests` block at the bottom of `bench.rs`:**

- `test_parse_comma_sizes_single`: `parse_comma_sizes("512")` → `Ok(vec![512])`.
- `test_parse_comma_sizes_multiple`: `parse_comma_sizes("128,256,512")` → `Ok(vec![128, 256, 512])`.
- `test_parse_comma_sizes_with_spaces`: `parse_comma_sizes("128, 256, 512")` → `Ok(vec![128, 256, 512])`.
- `test_parse_comma_sizes_invalid`: `parse_comma_sizes("abc")` → `Err(...)`.
- `test_parse_comma_sizes_empty`: `parse_comma_sizes("")` → `Err(...)`.

**Steps:**
- [ ] Write the 5 unit tests for `parse_comma_sizes` in `crates/koji-cli/src/handlers/bench.rs`
- [ ] Run `cargo test --package koji-cli --lib handlers::bench::tests`, confirm they fail (file doesn't exist yet)
- [ ] Create `crates/koji-cli/src/handlers/bench.rs` with `parse_comma_sizes`, `cmd_bench`, and the test module
- [ ] Add `pub mod bench;` to `crates/koji-cli/src/handlers/mod.rs`
- [ ] Add `Bench` variant to `Commands` enum in `crates/koji-cli/src/cli.rs`
- [ ] Add dispatch arm in `crates/koji-cli/src/lib.rs`
- [ ] Run `cargo test --package koji-cli --lib handlers::bench::tests`, confirm all 5 tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`, fix any warnings
- [ ] Run `cargo build --workspace`, confirm it succeeds
- [ ] Run `cargo build --release -p koji-cli` and verify `koji bench --help` shows the correct usage
- [ ] Commit with message: `feat: add koji bench CLI command`

**Acceptance criteria:**
- [ ] `koji bench --help` shows: name arg, --all, --pp, --tg, --runs, --warmup, --ctx flags
- [ ] `koji bench` with no args shows the error about specifying a name or --all
- [ ] `parse_comma_sizes` correctly handles valid input, spaces, and invalid input
- [ ] All 5 unit tests pass
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] `cargo test --workspace` passes all existing + new tests

---

### Task 5: Manual end-to-end verification and polish

**Context:**
This is a verification task. The feature is now fully wired up. This task runs `koji bench` against a real model to verify the full pipeline works end-to-end, fixes any issues found, and polishes the output. This task requires at least one model config to be set up in koji (the tester should use `koji model ls` to find one, or create a test config if none exist). This task also verifies error handling for common failure modes.

**Files:**
- Potentially modify: any file from Tasks 1-4 based on issues found

**What to verify:**

1. Run `koji model ls` to find an available model config name. If none, the tester should note this and skip to step 3.

2. Run `koji bench <model-config-name> --pp 128 --tg 32 --runs 1 --warmup 0` (small sizes for quick test). Verify:
   - Backend starts and health check passes (printed message)
   - Benchmark runs and produces a `RequestMeasurement` with non-zero values
   - Table output is formatted correctly with aligned columns
   - PP and TG values are reasonable (PP should be > TG for GPU inference)
   - Load time and VRAM are displayed
   - Backend is stopped cleanly (process is killed)

3. Verify error cases:
   - `koji bench nonexistent-model` → clear error message about model not found
   - `koji bench` (no args) → clear error about specifying name or --all
   - `koji bench --all` with no enabled models → clear error message
   - `koji bench <name> --pp abc` → clear error about invalid size

4. Run the full test suite: `cargo test --workspace` — all tests (existing + new) must pass.

5. Run `cargo clippy --workspace -- -D warnings` — no warnings.

6. Fix any issues found in steps 2-5. If fixes are needed, they should be minimal and targeted.

**Steps:**
- [ ] Run `cargo test --workspace`, confirm all tests pass
- [ ] Run `cargo clippy --workspace -- -D warnings`, confirm no warnings
- [ ] Run `koji bench --help`, verify help text is correct
- [ ] Run `koji bench` with no args, verify error message
- [ ] Run `koji bench nonexistent-model`, verify error message
- [ ] If a model config exists: run `koji bench <name> --pp 128 --tg 32 --runs 1 --warmup 0`, verify output
- [ ] Fix any issues found
- [ ] Run `cargo test --workspace` again to confirm no regressions
- [ ] Commit with message: `fix: polish bench command output and error handling`

**Acceptance criteria:**
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] Error messages are clear and actionable for all failure modes
- [ ] If tested with a real model: table output is correctly formatted and metrics are reasonable
