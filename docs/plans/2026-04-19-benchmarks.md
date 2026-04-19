# Benchmarks Page Plan

**Goal:** Add a web-based benchmarking page to koji-web that uses llama-bench to benchmark GGUF models with real-time progress via SSE, history persistence, and preset configurations.

**Architecture:** Extend the existing job system with a new `Benchmark` job kind in `koji-web/src/jobs.rs`. Create a new `koji-core/src/bench/llama_bench.rs` module that wraps the llama-bench binary. Add REST API endpoints in a new `api/benchmarks.rs` module. Build a Leptos page at `/benchmarks` with configuration UI, live SSE progress display, and results table. Persist benchmark results via a new SQLite migration (v13) and query module following the existing pattern (`metrics_queries.rs`, `backend_queries.rs`).

**Tech Stack:** Leptos 0.7 (WASM frontend), Axum (SSR backend), tokio (async subprocess), SQLite (history storage, migration-based), llama-bench binary (external tool).

---

### Task 1: Add SQLite Migration and Query Functions for Benchmark History

**Context:**
Koji uses a migration-based schema system in `migrations.rs` — new tables must be added as a new migration entry (incrementing `LATEST_VERSION`). Query functions live in separate files under `db/queries/` (e.g. `metrics_queries.rs`, `backend_queries.rs`). This task adds the benchmark history table and all CRUD query functions following this established pattern.

**Key references:**
- `crates/koji-core/src/db/migrations.rs` — migration list, `LATEST_VERSION` constant currently at 12
- `crates/koji-core/src/db/queries/mod.rs` — module re-exports
- `crates/koji-core/src/db/queries/metrics_queries.rs` — example of a simple table with insert + select queries

**Files:**
- Create: `crates/koji-core/src/db/queries/benchmark_queries.rs`
- Modify: `crates/koji-core/src/db/migrations.rs`
- Modify: `crates/koji-core/src/db/queries/mod.rs`

**What to implement:**

#### 1.1 New migration (v13) in `migrations.rs`:

Add a new entry to the migrations array, incrementing `LATEST_VERSION` from 12 to 13:

```rust
(
    13,
    r#"
        -- Stores benchmark results for comparison over time.
        CREATE TABLE IF NOT EXISTS benchmarks (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at          INTEGER NOT NULL,           -- Unix timestamp (seconds)
            model_id            TEXT NOT NULL,              -- Model config key (e.g. "qwen7b")
            display_name        TEXT,                       -- Model display name
            quant               TEXT,                       -- Quantization label (e.g. "Q4_K_M")
            backend             TEXT NOT NULL,              -- Backend type (e.g. "llama_cpp")
            engine              TEXT NOT NULL DEFAULT 'llama_bench',
            pp_sizes            TEXT NOT NULL,              -- JSON array, e.g. "[512,1024]"
            tg_sizes            TEXT NOT NULL,              -- JSON array, e.g. "[128,256]"
            threads             TEXT,                       -- JSON array or null
            ngl_range           TEXT,                       -- GPU layers range or null
            runs                INTEGER NOT NULL DEFAULT 3,
            warmup              INTEGER NOT NULL DEFAULT 1,
            results             TEXT NOT NULL,              -- JSON array of BenchSummary objects
            load_time_ms        REAL,                       -- Model load time in ms
            vram_used_mib       INTEGER,                    -- VRAM used at benchmark time
            vram_total_mib      INTEGER,                    -- Total VRAM
            duration_seconds    REAL,                       -- How long the benchmark took
            status              TEXT NOT NULL DEFAULT 'success'
        );
        CREATE INDEX IF NOT EXISTS idx_benchmarks_model_id ON benchmarks(model_id);
        CREATE INDEX IF NOT EXISTS idx_benchmarks_created_at ON benchmarks(created_at DESC);
    "#,
),
```

#### 1.2 Query module `benchmark_queries.rs`:

Create `crates/koji-core/src/db/queries/benchmark_queries.rs` with these functions following the `metrics_queries.rs` pattern:

```rust
//! Benchmark history database query functions.

use anyhow::{bail, Result};
use rusqlite::Connection;

/// Row from the benchmarks table.
#[derive(Debug, Clone)]
pub struct BenchmarkRow {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub engine: String,
    pub pp_sizes: String,   // JSON array string
    pub tg_sizes: String,   // JSON array string
    pub threads: Option<String>,  // JSON array string or null
    pub ngl_range: Option<String>,
    pub runs: u32,
    pub warmup: u32,
    pub results: String,    // JSON array string
    pub load_time_ms: Option<f64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub duration_seconds: f64,
    pub status: String,
}

/// Insert a benchmark result row. Returns the new row id.
pub fn insert_benchmark(
    conn: &Connection,
    model_id: &str,
    display_name: Option<&str>,
    quant: Option<&str>,
    backend: &str,
    engine: &str,
    pp_sizes_json: &str,
    tg_sizes_json: &str,
    threads_json: Option<&str>,
    ngl_range: Option<&str>,
    runs: u32,
    warmup: u32,
    results_json: &str,
    load_time_ms: Option<f64>,
    vram_used_mib: Option<i64>,
    vram_total_mib: Option<i64>,
    duration_seconds: f64,
    status: &str,
) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    let id = tx.execute(
        "INSERT INTO benchmarks (
            created_at, model_id, display_name, quant, backend, engine,
            pp_sizes, tg_sizes, threads, ngl_range, runs, warmup,
            results, load_time_ms, vram_used_mib, vram_total_mib,
            duration_seconds, status
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        (
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            model_id,
            display_name,
            quant,
            backend,
            engine,
            pp_sizes_json,
            tg_sizes_json,
            threads_json,
            ngl_range,
            runs as i64,
            warmup as i64,
            results_json,
            load_time_ms,
            vram_used_mib,
            vram_total_mib,
            duration_seconds,
            status,
        ),
    )?;
    tx.commit()?;
    Ok(id)
}

/// Fetch all benchmark entries ordered by created_at DESC.
pub fn list_benchmarks(conn: &Connection) -> Result<Vec<BenchmarkRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at, model_id, display_name, quant, backend, engine,
                pp_sizes, tg_sizes, threads, ngl_range, runs, warmup,
                results, load_time_ms, vram_used_mib, vram_total_mib,
                duration_seconds, status
         FROM benchmarks
         ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(BenchmarkRow {
            id: row.get(0)?,
            created_at: row.get(1)?,
            model_id: row.get(2)?,
            display_name: row.get(3)?,
            quant: row.get(4)?,
            backend: row.get(5)?,
            engine: row.get(6)?,
            pp_sizes: row.get(7)?,
            tg_sizes: row.get(8)?,
            threads: row.get(9)?,
            ngl_range: row.get(10)?,
            runs: row.get::<_, i64>(11)? as u32,
            warmup: row.get::<_, i64>(12)? as u32,
            results: row.get(13)?,
            load_time_ms: row.get(14)?,
            vram_used_mib: row.get(15)?,
            vram_total_mib: row.get(16)?,
            duration_seconds: row.get(17)?,
            status: row.get(18)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Delete a benchmark entry by id.
pub fn delete_benchmark(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM benchmarks WHERE id = ?1", [id])?;
    Ok(())
}
```

#### 1.3 Register in `queries/mod.rs`:

Add to the module declarations and re-exports:
```rust
mod benchmark_queries;
// ... existing modules ...
pub use benchmark_queries::*;
```

**Steps:**
- [ ] Read `crates/koji-core/src/db/queries/metrics_queries.rs` to understand the exact query pattern (parameter binding, transaction usage, query_map)
- [ ] Create `crates/koji-core/src/db/queries/benchmark_queries.rs` with all functions above
- [ ] Add `mod benchmark_queries;` and `pub use benchmark_queries::*;` to `crates/koji-core/src/db/queries/mod.rs`
- [ ] Increment `LATEST_VERSION` from 12 to 13 in `migrations.rs`
- [ ] Add the v13 migration entry to the migrations array in `migrations.rs`
- [ ] Run `cargo check --package koji-core`
  - Did it succeed? If not, fix the failures and re-run before continuing.
- [ ] Commit with message: "feat(db): add benchmarks table via migration v13"

**Acceptance criteria:**
- [ ] Migration v13 creates `benchmarks` table with all columns and indexes
- [ ] `insert_benchmark` inserts a row and returns the new id
- [ ] `list_benchmarks` returns entries ordered by `created_at DESC`
- [ ] `delete_benchmark` removes a row by id
- [ ] Migration is idempotent (running twice doesn't error)
- [ ] All existing DB tests still pass (`cargo test --package koji-core db`)

---

### Task 2: Create llama-bench Runner Module in koji-core

**Context:**
The existing `koji-core/src/bench/` module has core types (`BenchConfig`, `BenchReport`, `BenchSummary`, etc.) and an HTTP-based runner (`runner.rs`). We need a new submodule that wraps the llama-bench binary. This module detects the binary, builds CLI arguments, executes the benchmark, parses JSON output, and returns a `BenchReport`. It integrates with the existing `ProgressSink` trait for progress streaming.

**Key references:**
- `crates/koji-core/src/bench/mod.rs` — existing types (`BenchConfig`, `BenchReport`, `BenchSummary`, `ModelInfo`)
- `crates/koji-core/src/bench/runner.rs` — existing HTTP-based runner for patterns
- `crates/koji-core/src/backends/mod.rs` — `ProgressSink` trait definition
- `crates/koji-core/src/config/resolve/mod.rs` — `resolve_backend_path()` function

**Files:**
- Create: `crates/koji-core/src/bench/llama_bench.rs`
- Modify: `crates/koji-core/src/bench/mod.rs` (add `pub mod llama_bench;`)

**What to implement:**

New file `crates/koji-core/src/bench/llama_bench.rs`:

```rust
//! llama-bench integration for benchmarking GGUF files directly.
//!
//! Wraps the llama-bench binary from llama.cpp's tools/ directory.
//! Runs raw inference benchmarks without spawning a server.

use std::path::PathBuf;
use std::process::Stdio;
use anyhow::{Context, Result, bail};
use tokio::process::Command;
use crate::bench::{BenchConfig, BenchReport, BenchSummary, ModelInfo};
use crate::config::Config;
use crate::backends::ProgressSink;

/// Configuration for llama-bench specific parameters.
#[derive(Debug, Clone)]
pub struct LlamaBenchConfig {
    /// Prompt sizes to test (maps to -p)
    pub pp_sizes: Vec<u32>,
    /// Generation lengths to test (maps to -n)
    pub tg_sizes: Vec<u32>,
    /// Number of measurement runs (maps to -r)
    pub runs: u32,
    /// Warmup runs (handled by wrapper, not llama-bench itself)
    pub warmup: u32,
    /// Thread counts to test. None = auto-detect from system.
    pub threads: Option<Vec<u32>>,
    /// GPU layer range for sweet-spot sweep.
    /// Some("0-99+1") maps to --n-gpu-layers 0-99+1.
    /// None = use all layers (default).
    pub ngl_range: Option<String>,
    /// Optional context size override (maps to --fit-ctx)
    pub ctx_override: Option<u32>,
}

/// Locate the llama-bench binary.
/// Search order:
/// 1. LLAMA_BENCH_PATH environment variable
/// 2. llama.cpp tools/ directory relative to the backend binary
/// 3. llama-bench on PATH (system install via `which` equivalent)
pub fn find_llama_bench(backend_path: &std::path::Path) -> Result<PathBuf> {
    // 1. Check env var first
    if let Ok(p) = std::env::var("LLAMA_BENCH_PATH") {
        let p = std::path::PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }

    // 2. Check backend tools directory
    // Backend path is typically: /path/to/llama-server (binary)
    // Parent is: /path/to/ (bin dir)
    // Grandparent is: /path/to/llama.cpp/ or similar
    // Tools are at: <grandparent>/tools/llama-bench
    let grandparent = backend_path
        .parent()
        .and_then(|p| p.parent());

    if let Some(parent_dir) = grandparent {
        let tools_dir = parent_dir.join("tools");
        let bench_name = if cfg!(target_os = "windows") {
            "llama-bench.exe"
        } else {
            "llama-bench"
        };
        let bench_path = tools_dir.join(bench_name);
        if bench_path.exists() {
            return Ok(bench_path);
        }
    }

    // 3. Check PATH using std::env::split_paths + which equivalent
    let name = if cfg!(target_os = "windows") {
        "llama-bench.exe"
    } else {
        "llama-bench"
    };

    for path_dir in std::env::split_paths(&std::env::var("PATH").unwrap_or_default()) {
        let candidate = path_dir.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    bail!(
        "llama-bench binary not found. Install llama.cpp from source or set LLAMA_BENCH_PATH env var."
    )
}

/// Build the command-line arguments for llama-bench based on config.
fn build_args(
    model_path: &std::path::Path,
    config: &LlamaBenchConfig,
) -> Vec<String> {
    let mut args = Vec::new();

    // Model file(s) — llama-bench accepts -m for single model
    args.push("--model".to_string());
    args.push(model_path.to_string_lossy().to_string());

    // Prompt sizes: single value with -p, multiple comma-separated
    if config.pp_sizes.len() == 1 {
        args.push("-p".to_string());
        args.push(config.pp_sizes[0].to_string());
    } else {
        args.push("-p".to_string());
        args.push(config.pp_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","));
    }

    // Generation lengths: single value with -n, multiple comma-separated
    if config.tg_sizes.len() == 1 {
        args.push("-n".to_string());
        args.push(config.tg_sizes[0].to_string());
    } else {
        args.push("-n".to_string());
        args.push(config.tg_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","));
    }

    // Repetitions
    args.push("-r".to_string());
    args.push(config.runs.to_string());

    // Threads if specified
    if let Some(threads) = &config.threads {
        if threads.len() == 1 {
            args.push("--threads".to_string());
            args.push(threads[0].to_string());
        } else {
            args.push("--threads".to_string());
            args.push(threads.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","));
        }
    }

    // GPU layers range if specified (sweet spot sweep)
    if let Some(ref ngl_range) = config.ngl_range {
        args.push("--n-gpu-layers".to_string());
        args.push(ngl_range.clone());
    }

    // Context override
    if let Some(ctx) = config.ctx_override {
        args.push("--fit-ctx".to_string());
        args.push(ctx.to_string());
    }

    // Output format JSON (llama-bench -o json)
    args.push("-o".to_string());
    args.push("json".to_string());

    args
}

/// Parse llama-bench JSON output into a vector of BenchSummary.
///
/// llama-bench outputs an array where each entry has:
///   - n_prompt: prompt tokens (0 for pure TG tests)
///   - n_gen: generated tokens (0 for pure PP tests)
///   - avg_ts: average tokens/second
///   - stddev_ts: standard deviation
///   - model_filename, model_type, backend, etc.
fn parse_bench_json(output: &str) -> Result<Vec<BenchSummary>> {
    let entries: serde_json::Value = serde_json::from_str(output)
        .context("Failed to parse llama-bench JSON output")?;

    let arr = entries.as_array()
        .context("llama-bench JSON output is not an array")?;

    let mut summaries = Vec::new();

    for entry in arr {
        let n_prompt = entry["n_prompt"].as_u64().unwrap_or(0) as u32;
        let n_gen = entry["n_gen"].as_u64().unwrap_or(0) as u32;
        let avg_ts = entry["avg_ts"].as_f64().unwrap_or(0.0);
        let stddev_ts = entry["stddev_ts"].as_f64().unwrap_or(0.0);

        // Determine test type from the output
        let test_name = if n_prompt > 0 && n_gen == 0 {
            format!("pp{}", n_prompt)
        } else if n_prompt == 0 && n_gen > 0 {
            format!("tg{}", n_gen)
        } else if n_prompt > 0 && n_gen > 0 {
            // Prompt+gen combined test
            format!("pg{}-{}", n_prompt, n_gen)
        } else {
            "unknown".to_string()
        };

        // llama-bench reports a single avg_ts per entry.
        // For PP tests, this is the prompt processing speed.
        // For TG tests, this is the token generation speed.
        // llama-bench does NOT measure TTFT or total latency separately.
        let (pp_mean, pp_stddev, tg_mean, tg_stddev) = if n_prompt > 0 && n_gen == 0 {
            // Pure prompt processing test
            (avg_ts, stddev_ts, 0.0, 0.0)
        } else if n_prompt == 0 && n_gen > 0 {
            // Pure text generation test
            (0.0, 0.0, avg_ts, stddev_ts)
        } else {
            // Combined test — avg_ts is for the generation phase
            (0.0, 0.0, avg_ts, stddev_ts)
        };

        summaries.push(BenchSummary {
            test_name,
            prompt_tokens: n_prompt,
            gen_tokens: n_gen,
            pp_mean,
            pp_stddev,
            tg_mean,
            tg_stddev,
            ttft_mean: 0.0,    // llama-bench doesn't measure TTFT
            ttft_stddev: 0.0,
            total_mean: 0.0,    // llama-bench doesn't measure total latency
            total_stddev: 0.0,
        });
    }

    Ok(summaries)
}

/// Detect GPU type from backend binary path (same logic as existing runner).
fn detect_gpu_type(backend_path: &std::path::Path) -> String {
    let path_lower = backend_path.to_string_lossy().to_lowercase();
    if path_lower.contains("vulkan") {
        "Vulkan".to_string()
    } else if path_lower.contains("cuda") {
        "CUDA".to_string()
    } else if path_lower.contains("rocm") || path_lower.contains("hip") {
        "ROCm".to_string()
    } else if path_lower.contains("metal") {
        "Metal".to_string()
    } else if crate::gpu::query_vram().is_some() {
        "CUDA".to_string()
    } else {
        "CPU".to_string()
    }
}

/// Run a benchmark using llama-bench and return the report.
///
/// This function is designed to be called from a background job — it streams
/// progress via the provided ProgressSink.
pub async fn run_llama_bench(
    config: &Config,
    model_id: &str,
    bench_config: &LlamaBenchConfig,
    progress: &dyn ProgressSink,
) -> Result<BenchReport> {
    use crate::db::OpenResult;

    // Resolve the model config to get model path and backend info
    let db_dir = crate::config::Config::config_dir()?;
    let OpenResult { conn, .. } = crate::db::open(&db_dir)?;
    let model_configs = crate::db::load_model_configs(&conn)?;

    let (server_config, backend_config) = config.resolve_server(&model_configs, model_id)
        .context("Failed to resolve server config for benchmark")?;

    // Get the model file path from the model config's first quant file
    let model_path = {
        // Look up the model by repo_id (lowercase of model_id)
        let repo_id = model_id.to_lowercase().replace('/', "--");
        let record = crate::db::queries::get_model_config_by_repo_id(&conn, &repo_id)?;
        match record {
            Some(rec) => {
                // Find the first model file (kind='model') for this config
                let files = crate::db::queries::get_model_files(&conn, rec.id)?;
                files
                    .into_iter()
                    .find(|f| f.kind.as_deref() == Some("model"))
                    .map(|f| {
                        // Build full path: model storage dir + filename
                        let config_dir = db_dir;
                        // Model files are stored in the koji data directory
                        // The path is relative to the model's storage location
                        // We need to find the actual file — check common locations
                        let model_data_dir = config_dir.join("models");
                        let candidate = model_data_dir.join(&rec.repo_id).join(&f.filename);
                        if candidate.exists() {
                            candidate
                        } else {
                            // Fallback: try parent data dir
                            config_dir.join(&f.filename)
                        }
                    })
                    .context("No model file found for this config")?
            }
            None => bail!("Model config '{}' not found in database", model_id),
        }
    };

    // Resolve backend binary path
    let backend_path = {
        let conn = Config::open_db();
        config.resolve_backend_path(&server_config.backend, &conn)?
    };

    // Find llama-bench binary
    let bench_binary = find_llama_bench(&backend_path)
        .context("llama-bench not found")?;

    // Get llama-bench version for reporting
    let version_output = Command::new(&bench_binary)
        .arg("--version")
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().to_string().into())
        .unwrap_or_else(|| "unknown".to_string());

    progress.log(&format!("Using llama-bench: {}", bench_binary.display()));
    progress.log(&format!("Version: {}", version_output));
    progress.log(&format!("Model: {} ({})", model_id, model_path.display()));

    // Build command arguments
    let args = build_args(&model_path, bench_config);

    progress.log(&format!("Running: {} {}", bench_binary.display(), args.join(" ")));

    // Run llama-bench
    let start_time = std::time::Instant::now();

    let output = Command::new(&bench_binary)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute llama-bench")?;

    let duration = start_time.elapsed();

    // Stream stderr to progress
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        for line in stderr.lines() {
            if !line.trim().is_empty() {
                progress.log(line);
            }
        }
    }

    // Check exit status
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("llama-bench exited with error (code {}): {}", output.status, stderr);
    }

    // Parse JSON output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let summaries = parse_bench_json(&stdout)?;

    // Build model info
    let model_info = ModelInfo {
        name: model_id.to_string(),
        model_id: server_config.model.clone(),
        quant: server_config.quant.clone(),
        backend: server_config.backend.clone(),
        gpu_type: detect_gpu_type(&backend_path),
        context_length: bench_config.ctx_override.or(server_config.context_length),
        gpu_layers: None,
    };

    Ok(BenchReport {
        model_info,
        config: BenchConfig {
            pp_sizes: bench_config.pp_sizes.clone(),
            tg_sizes: bench_config.tg_sizes.clone(),
            runs: bench_config.runs,
            warmup: bench_config.warmup,
            ctx_override: bench_config.ctx_override,
        },
        summaries,
        load_time_ms: 0.0, // llama-bench doesn't measure load time separately
        vram: crate::gpu::query_vram(),
    })
}
```

**Steps:**
- [ ] Read `crates/koji-core/src/bench/runner.rs` to understand the existing runner patterns (how it resolves configs, spawns processes)
- [ ] Create `crates/koji-core/src/bench/llama_bench.rs` with all functions above
- [ ] Add `pub mod llama_bench;` to `crates/koji-core/src/bench/mod.rs`
- [ ] Run `cargo check --package koji-core`
  - Did it succeed? If not, fix the failures and re-run before continuing.
- [ ] Commit with message: "feat(bench): add llama-bench runner module"

**Acceptance criteria:**
- [ ] `find_llama_bench()` finds binary via env var, backend tools dir, or PATH
- [ ] `build_args()` produces correct CLI args for all config combinations
- [ ] `parse_bench_json()` correctly parses JSON output into `BenchSummary` objects
- [ ] `run_llama_bench()` orchestrates the full benchmark flow end-to-end
- [ ] Code compiles without warnings

---

### Task 3: Add Benchmark API Endpoints in koji-web

**Context:**
REST API endpoints that the frontend calls. These accept benchmark configuration, run llama-bench as a background job (using the existing job system), stream progress via SSE, and persist results to the database. Follows the pattern of `api/backends/jobs.rs` for job submission and SSE streaming.

**Key references:**
- `crates/koji-web/src/api/backends/jobs.rs` — job submission + SSE patterns
- `crates/koji-web/src/api/backends/types.rs` — DTO patterns, `job_to_active_dto` helper
- `crates/koji-web/Cargo.toml` — `async-stream` is already included in the `ssr` feature

**Files:**
- Create: `crates/koji-web/src/api/benchmarks.rs`
- Modify: `crates/koji-web/src/api.rs` (add `pub mod benchmarks;`)
- Modify: `crates/koji-web/src/server.rs` (add route imports and routes)

**What to implement:**

New file `crates/koji-web/src/api/benchmarks.rs`:

```rust
//! Benchmark API endpoints.
//!
//! Provides REST endpoints for triggering llama-bench benchmarks,
//! streaming progress via SSE, and managing benchmark history.

use axum::{
    extract::{State, Path},
    http::StatusCode,
    response::{IntoResponse, Json, Sse},
    Json as AxumJson,
};
use axum::response::Sse;
use futures::Stream;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::server::AppState;
use crate::jobs::{JobManager, JobKind, JobStatus};

// ── Request/Response DTOs ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkRunRequest {
    pub model_id: String,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub warmup: u32,
    #[serde(default)]
    pub threads: Option<Vec<u32>>,
    #[serde(default)]
    pub ngl_range: Option<String>,
    #[serde(default)]
    pub ctx_override: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkRunResponse {
    pub job_id: String,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkHistoryEntry {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub results_count: usize,
    pub status: String,
}

// ── Handler: Submit benchmark job ─────────────────────────────────────

pub async fn run_benchmark(
    State(state): State<Arc<AppState>>,
    AxumJson(req): AxumJson<BenchmarkRunRequest>,
) -> impl IntoResponse {
    let jobs = match &state.jobs {
        Some(j) => j.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Job manager not available"})),
            ).into_response();
        }
    };

    // Submit a benchmark job
    let job = match jobs.submit(JobKind::Benchmark, None).await {
        Ok(j) => j,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "Another job is already running"})),
            ).into_response();
        }
    };

    let job_id = job.id.clone();
    let req_clone = req.clone();
    let config_path = state.config_path.clone();

    // Spawn the benchmark in the background
    tokio::spawn(async move {
        if let Err(e) = run_benchmark_inner(&jobs, &job, &req_clone, &config_path).await {
            jobs.finish(&job, JobStatus::Failed, Some(e.to_string())).await;
        } else {
            jobs.finish(&job, JobStatus::Succeeded, None).await;
        }
    });

    (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
}

async fn run_benchmark_inner(
    jobs: &JobManager,
    job: &Arc<crate::jobs::Job>,
    req: &BenchmarkRunRequest,
    config_path: &Option<std::path::PathBuf>,
) -> Result<()> {
    use koji_core::bench::llama_bench::{self, LlamaBenchConfig};

    // Load config
    let config_dir = config_path.as_ref()
        .and_then(|p| p.parent())
        .context("Cannot determine config directory")?;

    let config = tokio::task::spawn_blocking(move || {
        koji_core::config::Config::load_from(config_dir)
    })
    .await??;

    // Create progress sink adapter (same pattern as backend install)
    let job_clone = job.clone();
    let jobs_clone = jobs.clone();
    struct BenchProgressSink {
        job: Arc<crate::jobs::Job>,
        jobs: Arc<JobManager>,
    }
    impl koji_core::backends::ProgressSink for BenchProgressSink {
        fn log(&self, line: &str) {
            let job = self.job.clone();
            let jobs = self.jobs.clone();
            let line = line.to_string();
            tokio::spawn(async move {
                jobs.append_log(&job, line).await;
            });
        }
    }

    let sink = BenchProgressSink {
        job: job_clone.clone(),
        jobs: jobs_clone.clone(),
    };

    // Build llama-bench config
    let bench_config = LlamaBenchConfig {
        pp_sizes: req.pp_sizes.clone(),
        tg_sizes: req.tg_sizes.clone(),
        runs: req.runs,
        warmup: req.warmup,
        threads: req.threads.clone(),
        ngl_range: req.ngl_range.clone(),
        ctx_override: req.ctx_override,
    };

    // Run benchmark
    let report = llama_bench::run_llama_bench(&config, &req.model_id, &bench_config, &sink).await?;

    // Store results in database
    let db_dir = koji_core::config::Config::config_dir()?;
    let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;

    // Get model display name from config
    let model_configs = koji_core::db::load_model_configs(&conn)?;
    let display_name = model_configs.get(&req.model_id)
        .and_then(|mc| mc.display_name.clone());

    // Serialize results to JSON string for storage
    let results_json = serde_json::to_string(&report.summaries)
        .context("Failed to serialize benchmark results")?;
    let pp_sizes_json = serde_json::to_string(&req.pp_sizes)
        .context("Failed to serialize pp_sizes")?;
    let tg_sizes_json = serde_json::to_string(&req.tg_sizes)
        .context("Failed to serialize tg_sizes")?;
    let threads_json = req.threads.as_ref()
        .map(|t| serde_json::to_string(t))
        .transpose()
        .context("Failed to serialize threads")?;

    // Get VRAM info
    let vram = crate::gpu::query_vram();

    // Insert into database
    let _id = koji_core::db::queries::insert_benchmark(
        &conn,
        &req.model_id,
        display_name.as_deref(),
        report.model_info.quant.as_deref(),
        &report.model_info.backend,
        "llama_bench",
        &pp_sizes_json,
        &tg_sizes_json,
        threads_json.as_deref(),
        req.ngl_range.as_deref(),
        req.runs,
        req.warmup,
        &results_json,
        Some(report.load_time_ms),
        vram.as_ref().map(|v| v.used_mib as i64),
        vram.as_ref().map(|v| v.total_mib as i64),
        0.0, // duration tracked by job system
        "success",
    )?;

    Ok(())
}

// ── Handler: Get benchmark result ─────────────────────────────────────

pub async fn get_benchmark_result(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let jobs = match &state.jobs {
        Some(j) => j.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Job manager not available"})),
            ).into_response();
        }
    };

    let job = match jobs.get(&job_id).await {
        Some(j) => j,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Job not found"})),
            ).into_response();
        }
    };

    let state = job.state.read().await;
    let error = state.error.clone();
    let status = format!("{:?}", state.status);
    drop(state);

    // Read log lines for context
    let log_lines: Vec<String> = {
        let head = job.log_head.read().await;
        let tail = job.log_tail.read().await;
        let mut lines: Vec<String> = head.iter().cloned().collect();
        lines.extend(tail.iter().cloned());
        lines
    };

    // For completed benchmarks, the results are in the DB, not the job log.
    // The frontend should fetch from /api/benchmarks/history for past runs.
    // For running/in-progress jobs, return empty results with status.

    (StatusCode::OK, Json(serde_json::json!({
        "job_id": job_id,
        "status": status,
        "error": error,
        "log_lines": log_lines,
    }))).into_response()
}

// ── Handler: SSE events for benchmark progress ────────────────────────

pub async fn benchmark_events(
    State(_state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let jobs = match &_state.jobs {
        Some(j) => j.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Job manager not available"})),
            ).into_response();
        }
    };

    let job = match jobs.get(&job_id).await {
        Some(j) => j,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Job not found"})),
            ).into_response();
        }
    };

    let rx = job.log_tx.subscribe();

    let stream = async_stream::stream! {
        // Send initial status
        let state = job.state.read().await;
        yield Ok::<_, axum::Error>(axum::response::Sse::new(serde_json::json!({
            "type": "status",
            "status": format!("{:?}", state.status),
        })).event("status"));
        drop(state);

        for await event in rx.iter() {
            match event {
                crate::jobs::JobEvent::Log(line) => {
                    yield Ok::<_, axum::Error>(axum::response::Sse::new(serde_json::json!({
                        "type": "log",
                        "line": line,
                    })).event("log"));
                }
                crate::jobs::JobEvent::Status(status) => {
                    yield Ok::<_, axum::Error>(axum::response::Sse::new(serde_json::json!({
                        "type": "status",
                        "status": format!("{:?}", status),
                    })).event("status"));
                }
            }
        }
    };

    Sse::new(stream).into_response()
}

// ── Handler: List benchmark history ───────────────────────────────────

pub async fn list_benchmark_history(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let db_dir = match koji_core::config::Config::config_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response();
        }
    };

    let entries = match tokio::task::spawn_blocking(move || {
        let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
        koji_core::db::queries::list_benchmarks(&conn)
    }).await {
        Ok(Ok(entries)) => entries,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response();
        }
    };

    let history: Vec<BenchmarkHistoryEntry> = entries.into_iter().map(|e| {
        let pp_sizes: Vec<u32> = serde_json::from_str(&e.pp_sizes).unwrap_or_default();
        let tg_sizes: Vec<u32> = serde_json::from_str(&e.tg_sizes).unwrap_or_default();
        BenchmarkHistoryEntry {
            id: e.id,
            created_at: e.created_at,
            model_id: e.model_id,
            display_name: e.display_name,
            quant: e.quant,
            backend: e.backend,
            pp_sizes,
            tg_sizes,
            runs: e.runs,
            results_count: serde_json::from_str::<Vec<_>>(&e.results).map(|v| v.len()).unwrap_or(0),
            status: e.status,
        }
    }).collect();

    Json(history).into_response()
}

// ── Handler: Delete benchmark history entry ───────────────────────────

pub async fn delete_benchmark(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let db_dir = match koji_core::config::Config::config_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response();
        }
    };

    match tokio::task::spawn_blocking(move || {
        let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
        koji_core::db::queries::delete_benchmark(&conn, id)
    }).await {
        Ok(Ok(())) => Json(serde_json::json!({"ok": true})).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ).into_response(),
    }
}
```

Modify `crates/koji-web/src/api.rs`:
- Add `pub mod benchmarks;` to the module declarations

Modify `crates/koji-web/src/server.rs`:
- Add import: `use crate::api::benchmarks::{run_benchmark, get_benchmark_result, benchmark_events, list_benchmark_history, delete_benchmark};`
- Add routes to the main router (before `.merge(backend_routes)`):
  ```rust
  .route("/api/benchmarks/run", post(run_benchmark))
  .route("/api/benchmarks/jobs/:id", get(get_benchmark_result))
  .route("/api/benchmarks/jobs/:id/events", get(benchmark_events))
  .route("/api/benchmarks/history", get(list_benchmark_history))
  .route("/api/benchmarks/history/:id", delete(delete_benchmark))
  ```

**Steps:**
- [ ] Read `crates/koji-web/src/api/backends/jobs.rs` to understand the job submission + SSE patterns
- [ ] Create `crates/koji-web/src/api/benchmarks.rs` with all handlers above
- [ ] Add `pub mod benchmarks;` to `crates/koji-web/src/api.rs`
- [ ] Add route imports and routes to `crates/koji-web/src/server.rs`
- [ ] Verify `async-stream`, `futures`, and `tokio-stream` are in the `ssr` feature deps of `crates/koji-web/Cargo.toml` (they should be — `async-stream` is already there, add `futures` and `tokio-stream` if missing)
- [ ] Run `cargo check --package koji-web --features ssr`
  - Did it succeed? If not, fix the failures and re-run before continuing.
- [ ] Commit with message: "feat(api): add benchmark REST API endpoints"

**Acceptance criteria:**
- [ ] `POST /api/benchmarks/run` accepts request, submits job, returns `job_id`
- [ ] `GET /api/benchmarks/jobs/:id` returns job status and log lines
- [ ] `GET /api/benchmarks/jobs/:id/events` streams SSE with "log" and "status" events
- [ ] `GET /api/benchmarks/history` returns past benchmark entries
- [ ] `DELETE /api/benchmarks/history/:id` removes a past run
- [ ] All code compiles without warnings

---

### Task 4: Add Benchmark Page and Components to koji-web Frontend

**Context:**
The Leptos frontend page at `/benchmarks`. Uses the same patterns as `pages/dashboard.rs`: SSE for real-time progress, actions for form submission, signals for state management. Must NOT use `chrono` (not available in WASM CSR target) — use `web_sys::Date` or simple string formatting instead.

**Key references:**
- `crates/koji-web/src/pages/dashboard.rs` — SSE connection, Action usage, signal patterns
- `crates/koji-web/src/components/sidebar.rs` — nav link pattern
- `crates/koji-web/Cargo.toml` — `web_sys` features already include `EventSource`, `HtmlSelectElement`, etc.

**Files:**
- Create: `crates/koji-web/src/pages/benchmarks.rs`
- Modify: `crates/koji-web/src/pages/mod.rs` (add `pub mod benchmarks;`)
- Modify: `crates/koji-web/src/lib.rs` (add `<Route path=path!("/benchmarks") view=pages::benchmarks::Benchmarks />`)
- Modify: `crates/koji-web/src/components/sidebar.rs` (add nav link)

**What to implement:**

New file `crates/koji-web/src/pages/benchmarks.rs`:

```rust
//! Benchmarks page — run llama-bench benchmarks from the web UI.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRequest {
    pub model_id: String,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub warmup: u32,
    pub threads: Option<Vec<u32>>,
    pub ngl_range: Option<String>,
    pub ctx_override: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub results_count: usize,
    pub status: String,
}

/// Preset configurations for quick benchmark setup.
#[derive(Debug, Clone)]
pub struct BenchmarkPreset {
    pub label: &'static str,
    pub pp_sizes: &'static [u32],
    pub tg_sizes: &'static [u32],
    pub runs: u32,
    pub threads: Option<Vec<u32>>,
    pub ngl_range: Option<&'static str>,
    pub ctx_override: Option<u32>,
}

impl BenchmarkPreset {
    pub fn all() -> Vec<Self> {
        vec![
            Self {
                label: "Quick",
                pp_sizes: &[512],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: None,
                ctx_override: None,
            },
            Self {
                label: "VRAM Sweet Spot",
                pp_sizes: &[512],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: Some("0-99+1"),
                ctx_override: None,
            },
            Self {
                label: "Thread Scaling",
                pp_sizes: &[64],
                tg_sizes: &[16],
                runs: 3,
                threads: Some(vec![1, 2, 4, 8, 16, 32]),
                ngl_range: None,
                ctx_override: None,
            },
        ]
    }
}

/// Format a Unix timestamp to "YYYY-MM-DD HH:MM" using web_sys (WASM-compatible).
fn format_timestamp(ts: i64) -> String {
    let date = web_sys::Date::new_with_seconds(ts as f64 * 1000.0);
    let month = (date.get_month() + 1) as u32; // months are 0-indexed
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        date.get_full_year(),
        month,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
    )
}

/// Format a stat as "mean ± stddev" or just "mean" if stddev is 0.
fn format_stat(mean: f64, stddev: f64) -> String {
    if stddev > 0.01 {
        format!("{:.1} ± {:.1}", mean, stddev)
    } else {
        format!("{:.1}", mean)
    }
}

#[component]
pub fn Benchmarks() -> impl IntoView {
    // Model selection
    let selected_model = RwSignal::new(String::new());
    let available_models = RwSignal::new(Vec::<(String, String, String)>::new()); // (id, display_name, quant)

    // Test configuration
    let pp_sizes_str = RwSignal::new("512".to_string());
    let tg_sizes_str = RwSignal::new("128".to_string());
    let runs = RwSignal::new(3u32);
    let warmup = RwSignal::new(1u32);
    let threads_str = RwSignal::new("auto".to_string());
    let ngl_range = RwSignal::new("".to_string());
    let ctx_override = RwSignal::new("".to_string());

    // Job state
    let is_running = RwSignal::new(false);
    let log_lines = RwSignal::new(Vec::<String>::new());
    let job_status = RwSignal::new(String::new());
    let has_results = RwSignal::new(false);
    let error_message = RwSignal::new(Option::<String>::None);

    // History state
    let history = RwSignal::new(Vec::<HistoryEntry>::new());
    let show_history = RwSignal::new(false);

    // Fetch available models on mount
    {
        let available_models = available_models;
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/koji/v1/models").send().await {
                if let Ok(models) = resp.json::<Vec<serde_json::Value>>().await {
                    let model_list: Vec<(String, String, String)> = models.iter().filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_string();
                        let name = m.get("display_name")
                            .or_else(|| m.get("api_name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(&id);
                        let quant = m.get("quant")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        Some((id, name.to_string(), quant.to_string()))
                    }).collect();
                    available_models.update(|list| *list = model_list);
                }
            }
        });
    }

    // Fetch benchmark history on mount
    {
        let history_signal = history;
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/api/benchmarks/history").send().await {
                if let Ok(entries) = resp.json::<Vec<HistoryEntry>>().await {
                    history_signal.set(entries);
                }
            }
        });
    }

    // Apply preset
    let apply_preset = move |preset: BenchmarkPreset| {
        pp_sizes_str.set(preset.pp_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","));
        tg_sizes_str.set(preset.tg_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","));
        runs.set(preset.runs);
        threads_str.set(
            preset.threads.map(|t| t.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","))
                .unwrap_or("auto".to_string())
        );
        ngl_range.set(preset.ngl_range.unwrap_or("").to_string());
    };

    // Connect to SSE for a given job_id
    let connect_to_sse = move |job_id: String| {
        let log_lines_signal = log_lines;
        let status_signal = job_status;
        let is_running_signal = is_running;
        let has_results_signal = has_results;
        let error_signal = error_message;

        spawn_local(async move {
            let es = match web_sys::EventSource::new(&format!("/api/benchmarks/jobs/{}/events", job_id)) {
                Ok(es) => es,
                Err(_) => return,
            };

            // Handle log events
            let on_log = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    if let Ok(event_json) = serde_json::from_str::<serde_json::Value>(&data_str) {
                        if event_json.get("type").and_then(|t| t.as_str()) == Some("log") {
                            if let Some(line) = event_json.get("line").and_then(|l| l.as_str()) {
                                log_lines_signal.update(|lines| {
                                    lines.push(line.to_string());
                                });
                            }
                        }
                    }
                }
            });
            let _ = es.add_event_listener_with_callback("log", on_log.as_ref().unchecked_ref());
            on_log.forget();

            // Handle status events
            let on_status = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    if let Ok(event_json) = serde_json::from_str::<serde_json::Value>(&data_str) {
                        if let Some(status) = event_json.get("status").and_then(|s| s.as_str()) {
                            status_signal.set(status.to_string());
                            let terminal = status == "Succeeded" || status == "Failed";
                            is_running_signal.set(!terminal);
                            has_results_signal.set(terminal);
                            if status == "Failed" {
                                error_signal.set(Some("Benchmark failed. Check logs above.".to_string()));
                            }
                        }
                    }
                }
            });
            let _ = es.add_event_listener_with_callback("status", on_status.as_ref().unchecked_ref());
            on_status.forget();

            on_cleanup(move || {
                es.close();
            });
        });
    };

    // Run benchmark action
    let run_action: Action<BenchmarkRequest, (), LocalStorage> = Action::new(move |req: &BenchmarkRequest| {
        let req_clone = req.clone();
        async move {
            // Submit benchmark job
            if let Ok(resp) = gloo_net::http::Request::post("/api/benchmarks/run")
                .json(&serde_json::json!({
                    "model_id": req_clone.model_id,
                    "pp_sizes": req_clone.pp_sizes,
                    "tg_sizes": req_clone.tg_sizes,
                    "runs": req_clone.runs,
                    "warmup": req_clone.warmup,
                    "threads": req_clone.threads,
                    "ngl_range": req_clone.ngl_range,
                    "ctx_override": req_clone.ctx_override,
                }))
                .send().await {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    if let Some(job_id) = body.get("job_id").and_then(|v| v.as_str()) {
                        // Connect to SSE for progress streaming
                        connect_to_sse(job_id.to_string());
                    }
                }
            }
        }
    });

    // Parse comma-separated strings into Vec<u32> for the request
    let parse_sizes = move |s: &str| -> Vec<u32> {
        s.split(',')
            .map(|v| v.trim().parse::<u32>().unwrap_or(0))
            .filter(|v| *v > 0)
            .collect()
    };

    // Parse threads string
    let parse_threads = move |s: &str| -> Option<Vec<u32>> {
        if s.trim().to_lowercase() == "auto" || s.trim().is_empty() {
            None
        } else {
            Some(s.split(',')
                .map(|v| v.trim().parse::<u32>().unwrap_or(0))
                .filter(|v| *v > 0)
                .collect())
        }
    };

    view! {
        <div class="page-header">
            <h1>"Benchmarks"</h1>
            <div class="flex-between gap-1">
                <button class="btn btn-secondary btn-sm" on:click=move |_| show_history.update(|v| *v = !*v)>
                    {move || if show_history.get() { "Hide History" } else { "Show History" }}
                </button>
            </div>
        </div>

        // Model selection
        <section class="card">
            <h3>"Model"</h3>
            <select
                class="form-select"
                on:change=move |e| {
                    let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                    selected_model.set(val);
                }
            >
                <option value="" disabled>"Select a model..."</option>
                {move || available_models.get().iter().map(|(id, name, quant)| {
                    let label = if !quant.is_empty() {
                        format!("{} ({})", name, quant)
                    } else {
                        name.clone()
                    };
                    view! {
                        <option value=id>{label}</option>
                    }.into_any()
                }).collect::<Vec<_>>()}
            </select>
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
                        prop:value=move || pp_sizes_str.get()
                        on:input=move |e| { pp_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 128,256,512"</small>
                </div>
                <div class="form-group">
                    <label>"Generation lengths (tokens)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || tg_sizes_str.get()
                        on:input=move |e| { tg_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 32,64,128"</small>
                </div>
                <div class="form-group">
                    <label>"Runs"</label>
                    <input
                        type="number"
                        class="form-control"
                        prop:value=move || runs.get()
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
                        prop:value=move || warmup.get()
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
                        prop:value=move || threads_str.get()
                        on:input=move |e| { threads_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"auto, or comma-separated e.g. 4,8,16"</small>
                </div>
                <div class="form-group">
                    <label>"GPU layers range (sweet spot)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || ngl_range.get()
                        on:input=move |e| { ngl_range.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"e.g. 0-99+1 to sweep, or empty for all"</small>
                </div>
            </div>
        </section>

        // Presets
        <section class="card">
            <h3>"Presets"</h3>
            <div class="preset-buttons">
                {BenchmarkPreset::all().into_iter().map(|preset| {
                    view! {
                        <button
                            class="btn btn-outline-secondary btn-sm"
                            on:click=move |_| apply_preset(preset.clone())
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
                prop:disabled=move || selected_model.get().is_empty() || is_running.get()
                on:click=move |_| {
                    let pp = parse_sizes(&pp_sizes_str.get());
                    let tg = parse_sizes(&tg_sizes_str.get());
                    let threads = parse_threads(&threads_str.get());
                    let ngl = if ngl_range.get().is_empty() { None } else { Some(ngl_range.get()) };
                    let ctx = if ctx_override.get().is_empty() { None } else {
                        ctx_override.get().parse::<u32>().ok()
                    };

                    let _ = run_action.dispatch(BenchmarkRequest {
                        model_id: selected_model.get(),
                        pp_sizes: pp,
                        tg_sizes: tg,
                        runs: runs.get(),
                        warmup: warmup.get(),
                        threads,
                        ngl_range: ngl,
                        ctx_override: ctx,
                    });
                }
            >
                {move || if is_running.get() { "Running..." } else { "▶ Run Benchmark" }}
            </button>
        </div>

        // Progress / logs
        {move || {
            if !log_lines.get().is_empty() {
                view! {
                    <section class="card">
                        <h3>"Progress"</h3>
                        <div class="log-panel">
                            {log_lines.get().into_iter().map(|line| {
                                view! {
                                    <pre class="log-line">{line}</pre>
                                }.into_any()
                            }).collect::<Vec<_>>()}
                        </div>
                    </section>
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}

        // Error message
        {move || {
            if let Some(err) = error_message.get() {
                view! {
                    <div class="alert alert-danger mt-3">{err}</div>
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}

        // History
        {move || {
            if show_history.get() && !history.get().is_empty() {
                view! {
                    <section class="card mt-3">
                        <h3>"Benchmark History"</h3>
                        <table class="table table-striped">
                            <thead>
                                <tr>
                                    <th>"Date"</th>
                                    <th>"Model"</th>
                                    <th>"Quant"</th>
                                    <th>"PP sizes"</th>
                                    <th>"TG sizes"</th>
                                    <th>"Results"</th>
                                    <th>"Status"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {history.get().into_iter().map(|entry| {
                                    let date = format_timestamp(entry.created_at);
                                    let badge_class = if entry.status == "success" { "badge badge-success" } else { "badge badge-danger" };
                                    view! {
                                        <tr>
                                            <td>{date}</td>
                                            <td>{entry.model_id}</td>
                                            <td>{entry.quant.unwrap_or_else(|| "—".to_string())}</td>
                                            <td class="text-mono">{entry.pp_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")}</td>
                                            <td class="text-mono">{entry.tg_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")}</td>
                                            <td>{entry.results_count}</td>
                                            <td><span class={badge_class}>{entry.status}</span></td>
                                        </tr>
                                    }.into_any()
                                }).collect::<Vec<_>>()}
                            </tbody>
                        </table>
                    </section>
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}
    }
}
```

Modify `crates/koji-web/src/pages/mod.rs`:
- Add `pub mod benchmarks;`

Modify `crates/koji-web/src/lib.rs`:
- Add route: `<Route path=path!("/benchmarks") view=pages::benchmarks::Benchmarks />`

Modify `crates/koji-web/src/components/sidebar.rs`:
- Add nav link between "Downloads" and "Config":
  ```html
  <A href="/benchmarks" attr:class="sidebar-item" attr:data-tooltip="Benchmarks" on:click=move |_| mobile_open.set(false)>
      <span class="sidebar-item__icon">"📊"</span>
      <span class="sidebar-item__text">"Benchmarks"</span>
  </A>
  ```

**Steps:**
- [ ] Read `crates/koji-web/src/pages/dashboard.rs` to understand Leptos patterns used in the project (SSE connection, Action usage, signal management)
- [ ] Create `crates/koji-web/src/pages/benchmarks.rs` with the full component
- [ ] Add `pub mod benchmarks;` to `crates/koji-web/src/pages/mod.rs`
- [ ] Add route to `crates/koji-web/src/lib.rs`
- [ ] Add nav link to `crates/koji-web/src/components/sidebar.rs`
- [ ] Run `cargo check --package koji-web` (for CSR) and `cargo check --package koji-web --features ssr` (for SSR)
  - Did it succeed? If not, fix the failures and re-run before continuing.
- [ ] Commit with message: "feat(web): add benchmarks page with configuration UI and results display"

**Acceptance criteria:**
- [ ] `/benchmarks` route renders the benchmark page
- [ ] Sidebar shows "Benchmarks" nav link with 📊 icon
- [ ] Model dropdown populates from `/koji/v1/models` API
- [ ] Preset buttons correctly update all configuration fields
- [ ] Run button is disabled when no model selected
- [ ] SSE connection streams log lines to the progress panel
- [ ] History section shows past benchmark runs with formatted dates
- [ ] No WASM compilation errors (CSR build succeeds)

---

### Task 5: Add CSS Styles for Benchmarks Page

**Context:**
The benchmarks page needs specific styling for preset buttons, log panel, results table, and grid layout. Find the existing CSS file and add benchmark-specific styles following the project's naming conventions.

**Files:**
- Find the CSS file by searching for `*.css` in `crates/koji-web/src/` or `crates/koji-web/dist/`
- Modify: that CSS file

**What to implement:**
Add CSS rules for:
1. `.preset-buttons` — flex-wrap row of outline buttons with spacing
2. `.log-panel` — monospace scrollable text area with dark background, fixed height
3. `.log-line` — individual log lines with proper padding/margin
4. `.grid-2` — two-column grid for form inputs (if not already defined)
5. Benchmark-specific table styling

**Steps:**
- [ ] Find the project's CSS file: `find crates/koji-web/src -name "*.css" -o -name "*.scss"` or check `dist/` directory
- [ ] Read the existing CSS to understand naming conventions and CSS variable usage
- [ ] Add benchmark-specific styles following the existing patterns
- [ ] Run `cargo check --package koji-web` to verify nothing broke
- [ ] Commit with message: "style: add benchmarks page CSS"

**Acceptance criteria:**
- [ ] Preset buttons display in a wrapped flex row with proper spacing
- [ ] Log panel has dark background, monospace font, scrollable overflow, fixed height (~300px)
- [ ] Benchmark results table uses striped rows
- [ ] No CSS conflicts with existing styles

---

### Task 6: Integration Tests and End-to-End Verification

**Context:**
Verify that all pieces work together: DB migration creates correctly, llama-bench runner detects the binary, API endpoints respond properly, and the frontend renders and interacts correctly.

**Files:**
- Modify: `crates/koji-core/src/db/queries/benchmark_queries.rs` (add tests)
- Modify: `crates/koji-core/src/bench/llama_bench.rs` (add tests)

**What to implement:**
Unit tests for the new DB functions and the llama-bench argument builder.

**Steps:**
- [ ] Add tests to `crates/koji-core/src/db/queries/benchmark_queries.rs`:
  - Test `insert_benchmark` + `list_benchmarks` round-trip
  - Test `delete_benchmark` removes entry
  - Use `open_in_memory()` for in-memory SQLite testing
- [ ] Add tests to `crates/koji-core/src/bench/llama_bench.rs`:
  - Test `build_args` with single values (should use `-p 512`, not `-p 512,512`)
  - Test `build_args` with multiple values (should use `-p 128,256,512`)
  - Test `build_args` with threads Some vs None
  - Test `build_args` with ngl_range Some vs None
  - Test `parse_bench_json` with sample llama-bench output
- [ ] Run `cargo test --package koji-core bench` and `cargo test --package koji-core db::queries::benchmark`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "test: add benchmarks module tests"

**Acceptance criteria:**
- [ ] All unit tests pass
- [ ] `build_args()` produces correct output for all config combinations
- [ ] DB functions work correctly with in-memory SQLite
- [ ] No new warnings from `cargo clippy --workspace`

---

## Summary

| Task | Description | Files Changed |
|------|-------------|---------------|
| 1 | SQLite migration v13 + benchmark query functions | `migrations.rs`, `benchmark_queries.rs`, `queries/mod.rs` |
| 2 | llama-bench runner module | `llama_bench.rs`, `bench/mod.rs` |
| 3 | Benchmark REST API endpoints | `benchmarks.rs`, `api.rs`, `server.rs` |
| 4 | Benchmarks page frontend | `benchmarks.rs`, `pages/mod.rs`, `lib.rs`, `sidebar.rs` |
| 5 | CSS styles | CSS file (to be discovered) |
| 6 | Tests and verification | `benchmark_queries.rs`, `llama_bench.rs` |

**Total estimated tasks:** 6
**Dependencies:** Task 1 → Task 3, Task 2 → Task 3, Task 3 → Task 4, Task 4 → Task 5
