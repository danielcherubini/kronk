//! llama-server speculative decoding benchmark module.
//!
//! Spawns a `llama-server` process with the appropriate speculative decoding
//! flags (`--spec-type`, `--spec-draft-n-max`, `--spec-draft-n-min`, and
//! type-specific `--spec-ngram-*-size-n/m` / `--spec-ngram-mod-n-match/min/max`)
//! and makes HTTP completion requests to the running server to measure throughput.
//!
//! Split into:
//! - [`server`] — llama-server process lifecycle and HTTP API client.
//! - [`discovery`] — binary lookup (pure filesystem logic).
//!
//! This module's `mod.rs` keeps the public types ([`SpecType`],
//! [`SpecBenchConfig`], [`SpecEntry`], [`SpecBenchResult`]) and the async
//! orchestrator [`run_spec_bench`].

mod discovery;
mod server;

pub use discovery::find_llama_server;

use std::sync::Arc;

use crate::backends::ProgressSink;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Speculative decoding type (maps to --spec-type CLI flag).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
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

    /// Returns the type-specific n-gram CLI flags (llama.cpp PR #22397).
    ///
    /// Each spec type has its own set of parameter flags:
    /// - `ngram-simple`: `--spec-ngram-simple-size-n`, `--spec-ngram-simple-size-m`, `--spec-ngram-simple-min-hits`
    /// - `ngram-mod`: `--spec-ngram-mod-n-match` (no size-m or min-hits)
    /// - `ngram-map-k`: `--spec-ngram-map-k-size-n`, `--spec-ngram-map-k-size-m`, `--spec-ngram-map-k-min-hits`
    /// - `ngram-map-k4v`: `--spec-ngram-map-k4v-size-n`, `--spec-ngram-map-k4v-size-m`, `--spec-ngram-map-k4v-min-hits`
    ///
    /// Returns `(size_n_flag, size_m_flag, min_hits_flag)` — empty strings for flags
    /// that don't apply to this spec type (e.g., ngram-mod has no size-m or min-hits).
    pub fn spec_ngram_flags(&self) -> (&'static str, &'static str, &'static str) {
        match self {
            SpecType::NgramSimple => (
                "--spec-ngram-simple-size-n",
                "--spec-ngram-simple-size-m",
                "--spec-ngram-simple-min-hits",
            ),
            SpecType::NgramMod => ("--spec-ngram-mod-n-match", "", ""),
            SpecType::NgramMapK => (
                "--spec-ngram-map-k-size-n",
                "--spec-ngram-map-k-size-m",
                "--spec-ngram-map-k-min-hits",
            ),
            SpecType::NgramMapK4v => (
                "--spec-ngram-map-k4v-size-n",
                "--spec-ngram-map-k4v-size-m",
                "--spec-ngram-map-k4v-min-hits",
            ),
        }
    }
}

/// Configuration for a speculative decoding benchmark sweep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecBenchConfig {
    /// Paths to the target model GGUF file.
    pub model_path: PathBuf,
    /// Spec types to test (e.g. [NgramSimple, NgramMod]).
    pub spec_types: Vec<SpecType>,
    /// Draft max values to sweep (e.g. [8, 16, 32, 64]).
    pub draft_max_values: Vec<u32>,
    /// N-gram lookup size N values for ngram-mod and ngram-map-* types.
    pub ngram_n_values: Vec<u32>,
    /// N-gram draft size M values for ngram-map-* types.
    pub ngram_m_values: Vec<u32>,
    /// N-gram minimum match values for n-gram-mod (e.g. [3, 5]).
    #[serde(default)]
    pub ngram_min_values: Vec<u32>,
    /// N-gram maximum match values for n-gram-mod (e.g. [48, 64]).
    #[serde(default)]
    pub ngram_max_values: Vec<u32>,
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

fn default_min_hits() -> u32 {
    1
}
fn default_gen_tokens() -> u32 {
    256
}
fn default_runs() -> u32 {
    3
}
fn default_flash_attn() -> bool {
    true
}

/// Result of a single spec-decoding config test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecEntry {
    pub spec_type: String,
    pub draft_max: u32,
    /// N-gram lookup size (maps to `--spec-ngram-*-size-n` or `--spec-ngram-mod-n-match`). None for ngram-simple.
    pub ngram_n: Option<u32>,
    /// N-gram draft size (only for ngram-map-*). None for others.
    pub ngram_m: Option<u32>,
    /// N-gram minimum match (only for n-gram-mod). None for other types.
    pub ngram_min: Option<u32>,
    /// N-gram maximum match (only for n-gram-mod). None for other types.
    pub ngram_max: Option<u32>,
    /// Mean token generation speed (tokens/s).
    pub tg_ts_mean: f64,
    /// Stddev of token generation speed.
    pub tg_ts_stddev: f64,
    /// Percentage delta vs baseline. Positive = faster, negative = slower.
    /// Formula: ((tg_ts_mean - baseline_tg_ts) / baseline_tg_ts) * 100
    pub delta_pct: f64,
    /// Draft acceptance rate from server statistics (0.0–1.0). None if not available.
    pub acceptance_rate: Option<f64>,
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

/// A single sweep configuration to test.
#[derive(Debug, Clone)]
struct SweepConfig {
    spec_type: SpecType,
    draft_max: u32,
    ngram_n: Option<u32>,
    ngram_m: Option<u32>,
    ngram_min: Option<u32>,
    ngram_max: Option<u32>,
}

/// Validate a [`SpecBenchConfig`] would produce at least one sweep entry.
///
/// Checks that required dimensions (e.g. `ngram_n_values` for ngram-mod) are
/// populated for the selected spec-types, and that the sweep is not empty.
pub fn validate_sweep_config(config: &SpecBenchConfig) -> Result<()> {
    let matrix = build_sweep_matrix(config)?;
    if matrix.is_empty() {
        bail!(
            "Sweep would produce zero entries. Ensure draft_max_values is not empty and required ngram dimensions are populated."
        );
    }
    Ok(())
}

/// Build the sweep matrix of configurations to test.
///
/// Returns an error if required dimensions are not populated for the selected spec-types.
fn build_sweep_matrix(config: &SpecBenchConfig) -> Result<Vec<SweepConfig>> {
    let spec_types = &config.spec_types;

    let needs_n = spec_types.iter().any(|t| {
        matches!(
            t,
            SpecType::NgramMod | SpecType::NgramMapK | SpecType::NgramMapK4v
        )
    });
    let needs_m = spec_types
        .iter()
        .any(|t| matches!(t, SpecType::NgramMapK | SpecType::NgramMapK4v));
    let needs_minmax = spec_types.iter().any(|t| matches!(t, SpecType::NgramMod));

    if needs_n && config.ngram_n_values.is_empty() {
        bail!("ngram_n_values is required when testing ngram-mod or ngram-map-* types");
    }
    if needs_m && config.ngram_m_values.is_empty() {
        bail!("ngram_m_values is required when testing ngram-map-k or ngram-map-k4v types");
    }
    if needs_minmax && (config.ngram_min_values.is_empty() || config.ngram_max_values.is_empty()) {
        bail!("ngram_min_values and ngram_max_values are required when testing n-gram-mod");
    }

    let mut matrix = Vec::new();

    for &st in spec_types {
        match st {
            SpecType::NgramMod => {
                // ngram-mod draft length is controlled by n-min/n-max,
                // not --spec-draft-n-max. Sweep only n-match/n-min/n-max.
                // (Use first draft_max value as a non-binding ceiling.)
                let dm = config.draft_max_values.first().copied().unwrap_or(16);
                for &nn in &config.ngram_n_values {
                    for &nm in &config.ngram_min_values {
                        for &nxm in &config.ngram_max_values {
                            matrix.push(SweepConfig {
                                spec_type: st,
                                draft_max: dm,
                                ngram_n: Some(nn),
                                ngram_m: None,
                                ngram_min: Some(nm),
                                ngram_max: Some(nxm),
                            });
                        }
                    }
                }
            }
            _ => {
                for &dm in &config.draft_max_values {
                    match st {
                        SpecType::NgramSimple => {
                            matrix.push(SweepConfig {
                                spec_type: st,
                                draft_max: dm,
                                ngram_n: None,
                                ngram_m: None,
                                ngram_min: None,
                                ngram_max: None,
                            });
                        }
                        SpecType::NgramMapK | SpecType::NgramMapK4v => {
                            for &nn in &config.ngram_n_values {
                                for &nm in &config.ngram_m_values {
                                    matrix.push(SweepConfig {
                                        spec_type: st,
                                        draft_max: dm,
                                        ngram_n: Some(nn),
                                        ngram_m: Some(nm),
                                        ngram_min: None,
                                        ngram_max: None,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(matrix)
}

/// Compute mean and population stddev from a slice of f64 values.
fn compute_mean_stddev(values: &[f64]) -> (f64, f64) {
    let count = values.len();
    if count == 0 {
        return (0.0, 0.0);
    }

    let mean = values.iter().sum::<f64>() / count as f64;

    let stddev = if count == 1 {
        0.0
    } else {
        let variance: f64 = values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / count as f64;
        variance.sqrt()
    };

    (mean, stddev)
}

/// Format a SweepConfig as CLI-style label for log output.
/// e.g. `--spec-type ngram-mod --spec-draft-n-max 8 --spec-ngram-mod-n-match 16 --spec-ngram-mod-n-min 3 --spec-ngram-mod-n-max 48`
fn format_config_label(cfg: &SweepConfig) -> String {
    let (flag_n, flag_m, _flag_hits) = cfg.spec_type.spec_ngram_flags();
    let mut parts = vec![
        format!("--spec-type {}", cfg.spec_type.as_str()),
        format!("--spec-draft-n-max {}", cfg.draft_max),
    ];
    if !flag_n.is_empty() {
        if let Some(n) = cfg.ngram_n {
            parts.push(format!("{} {}", flag_n, n));
        }
    }
    if !flag_m.is_empty() {
        if let Some(m) = cfg.ngram_m {
            parts.push(format!("{} {}", flag_m, m));
        }
    }
    if matches!(cfg.spec_type, SpecType::NgramMod) {
        if let Some(nmin) = cfg.ngram_min {
            parts.push(format!("--spec-ngram-mod-n-min {}", nmin));
        }
        if let Some(nmax) = cfg.ngram_max {
            parts.push(format!("--spec-ngram-mod-n-max {}", nmax));
        }
    }
    parts.join(" ")
}

/// Find an available port by binding to port 0.
async fn find_available_port() -> Result<u16> {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    Ok(addr.port())
}

/// Execute benchmark runs against a running llama-server.
///
/// Makes `config.runs` completion requests and returns timing stats.
/// Parses draft acceptance rate from server stderr statistics.
/// If a run produces an impossibly fast result (>10x baseline), it is logged
/// and re-run (up to 3 retries) before being accepted.
async fn execute_server_runs(
    handle: &server::ServerHandle,
    sweep_cfg: &SweepConfig,
    bench_cfg: &SpecBenchConfig,
    baseline_mean: f64,
    progress: Arc<dyn ProgressSink>,
) -> SpecEntry {
    let label = format_config_label(sweep_cfg);
    let prompt = crate::bench::build_prompt(512);
    let mut timings = Vec::new();
    const MAX_WILD_RETRIES: u32 = 3;

    for run in 1..=bench_cfg.runs {
        let mut retries = 0;
        loop {
            progress.log(&format!(
                "[{}] run {}/{}{}",
                label,
                run,
                bench_cfg.runs,
                if retries > 0 {
                    format!(" (retry {})", retries)
                } else {
                    String::new()
                }
            ));

            match handle.complete(&prompt, bench_cfg.gen_tokens).await {
                Ok(tokens_per_sec) => {
                    // Check for impossibly fast result (>10x baseline)
                    if baseline_mean > 0.0 && tokens_per_sec > baseline_mean * 10.0 {
                        progress.log(&format!(
                            "[{}] run {} wild result: {:.2} tokens/s is {:.0}x baseline ({:.2}) — discarding",
                            label, run, tokens_per_sec, tokens_per_sec / baseline_mean, baseline_mean
                        ));
                        if retries >= MAX_WILD_RETRIES {
                            progress.log(&format!(
                                "[{}] run {} accepted after {} retries (may be outlier)",
                                label, run, MAX_WILD_RETRIES
                            ));
                            timings.push(tokens_per_sec);
                            break;
                        }
                        retries += 1;
                        continue;
                    }
                    timings.push(tokens_per_sec);
                    break;
                }
                Err(e) => {
                    progress.log(&format!("[{}] run {} failed: {}", label, run, e));
                    return SpecEntry {
                        spec_type: sweep_cfg.spec_type.as_str().to_string(),
                        draft_max: sweep_cfg.draft_max,
                        ngram_n: sweep_cfg.ngram_n,
                        ngram_m: sweep_cfg.ngram_m,
                        ngram_min: sweep_cfg.ngram_min,
                        ngram_max: sweep_cfg.ngram_max,
                        tg_ts_mean: 0.0,
                        tg_ts_stddev: 0.0,
                        delta_pct: 0.0,
                        acceptance_rate: None,
                        status: "failed".to_string(),
                        error: Some(e.to_string()),
                    };
                }
            }
        }
    }

    let (mean, stddev) = compute_mean_stddev(&timings);
    let acceptance_rate = handle.parse_acceptance_rate().await;
    progress.log(&format!(
        "[{}] completed: {:.2} ± {:.2} tokens/s (acceptance: {:?})",
        label, mean, stddev, acceptance_rate
    ));

    SpecEntry {
        spec_type: sweep_cfg.spec_type.as_str().to_string(),
        draft_max: sweep_cfg.draft_max,
        ngram_n: sweep_cfg.ngram_n,
        ngram_m: sweep_cfg.ngram_m,
        ngram_min: sweep_cfg.ngram_min,
        ngram_max: sweep_cfg.ngram_max,
        tg_ts_mean: mean,
        tg_ts_stddev: stddev,
        delta_pct: 0.0,
        acceptance_rate,
        status: "success".to_string(),
        error: None,
    }
}

/// Spawn a server for a single config, execute benchmark runs, and return results.
///
/// Each config gets its own server to ensure correct parameters (ngram params
/// and draft_max are server startup flags that can't be changed mid-session).
async fn run_single_config(
    binary: &Path,
    cfg: &SweepConfig,
    bench_cfg: &SpecBenchConfig,
    baseline_mean: f64,
    progress: Arc<dyn ProgressSink>,
) -> SpecEntry {
    let label = format_config_label(cfg);

    let port = match find_available_port().await {
        Ok(p) => p,
        Err(e) => {
            progress.log(&format!("Failed to find available port: {}", e));
            return SpecEntry {
                spec_type: cfg.spec_type.as_str().to_string(),
                draft_max: cfg.draft_max,
                ngram_n: cfg.ngram_n,
                ngram_m: cfg.ngram_m,
                ngram_min: cfg.ngram_min,
                ngram_max: cfg.ngram_max,
                tg_ts_mean: 0.0,
                tg_ts_stddev: 0.0,
                delta_pct: 0.0,
                acceptance_rate: None,
                status: "failed".to_string(),
                error: Some(format!("Port allocation failed: {}", e)),
            };
        }
    };

    // draft_min/draft_max are not used for ngram-mod (draft length controlled by n-min/n-max)
    let use_draft_bounds = !matches!(cfg.spec_type, SpecType::NgramMod);
    let draft_max_val = use_draft_bounds.then_some(cfg.draft_max);
    let draft_min_val = use_draft_bounds.then_some((cfg.draft_max / 2).max(1));

    let server_args = server::ServerArgs {
        binary: binary.to_path_buf(),
        model_path: bench_cfg.model_path.clone(),
        port,
        ngl: bench_cfg.ngl,
        flash_attn: bench_cfg.flash_attn,
        spec_type: Some(cfg.spec_type),
        spec_ngram_n: cfg.ngram_n,
        spec_ngram_m: cfg.ngram_m,
        spec_ngram_min_hits: (bench_cfg.ngram_min_hits > 1).then_some(bench_cfg.ngram_min_hits),
        spec_ngram_min: cfg.ngram_min,
        spec_ngram_max: cfg.ngram_max,
        draft_max: draft_max_val,
        draft_min: draft_min_val,
    };

    let arg_vec = server_args.to_args();
    progress.log(&format!(
        "Starting llama-server on port {} ({})",
        port, label
    ));
    progress.log(&format!(
        "llama-server {} {}",
        binary.display(),
        arg_vec.join(" ")
    ));

    let timeout_secs = std::env::var("LLAMA_SERVER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300);

    let handle = match server::spawn_server(&server_args, timeout_secs).await {
        Ok(h) => h,
        Err(e) => {
            progress.log(&format!(
                "Failed to start llama-server for {}: {}",
                label, e
            ));
            return SpecEntry {
                spec_type: cfg.spec_type.as_str().to_string(),
                draft_max: cfg.draft_max,
                ngram_n: cfg.ngram_n,
                ngram_m: cfg.ngram_m,
                ngram_min: cfg.ngram_min,
                ngram_max: cfg.ngram_max,
                tg_ts_mean: 0.0,
                tg_ts_stddev: 0.0,
                delta_pct: 0.0,
                acceptance_rate: None,
                status: "failed".to_string(),
                error: Some(format!("Server start failed: {}", e)),
            };
        }
    };

    progress.log(&format!("llama-server ready on port {} ({})", port, label));

    execute_server_runs(&handle, cfg, bench_cfg, baseline_mean, progress.clone()).await
}

/// Run a speculative decoding benchmark sweep using llama-server.
///
/// Spawns one `llama-server` per spec-type group (since spec-type is a server
/// startup flag). Each server handles all draft-max variants for its type.
///
/// # Arguments
/// - `config`: benchmark configuration specifying model, spec types, sweep dimensions.
/// - `binary_override`: optional path to the `llama-server` binary. If `None`, uses
///   discovery to find it alongside the backend's `llama-server` binary.
/// - `progress`: progress sink for streaming status updates.
///
/// # Returns
/// A [`SpecBenchResult`] with baseline timing and one entry per sweep configuration.
pub async fn run_spec_bench(
    config: &SpecBenchConfig,
    binary_override: Option<PathBuf>,
    progress: Arc<dyn ProgressSink>,
) -> Result<SpecBenchResult> {
    // Step 1: Discover or use provided llama-server binary.
    let backend_dir = config
        .model_path
        .parent()
        .unwrap_or(std::path::Path::new(""));
    let binary = if let Some(bp) = binary_override {
        if !bp.exists() {
            bail!(
                "Provided llama-server path does not exist: {}",
                bp.display()
            );
        }
        bp
    } else {
        discovery::find_llama_server(backend_dir)
            .context("llama-server not found. Set LLAMA_SERVER_PATH or ensure llama-server is in the backend directory.")?
    };

    progress.log(&format!("Using llama-server: {}", binary.display()));
    progress.log(&format!(
        "Model: {} (gen_tokens={}, runs={})",
        config.model_path.display(),
        config.gen_tokens,
        config.runs,
    ));

    // Step 2: Run baseline (no spec-decoding) on a dedicated server.
    progress.log("Starting baseline server (no speculative decoding)...");
    let baseline_port = find_available_port().await?;
    let baseline_args = server::ServerArgs {
        binary: binary.clone(),
        model_path: config.model_path.clone(),
        port: baseline_port,
        ngl: config.ngl,
        flash_attn: config.flash_attn,
        spec_type: None,
        spec_ngram_n: None,
        spec_ngram_m: None,
        spec_ngram_min_hits: None,
        spec_ngram_min: None,
        spec_ngram_max: None,
        draft_max: None,
        draft_min: None,
    };
    progress.log(&format!(
        "llama-server {} {}",
        binary.display(),
        baseline_args.to_args().join(" ")
    ));

    let timeout_secs = std::env::var("LLAMA_SERVER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300);

    let baseline_handle = server::spawn_server(&baseline_args, timeout_secs)
        .await
        .with_context(|| "Failed to start baseline llama-server")?;

    progress.log(&format!("Baseline server ready on port {}", baseline_port));

    let mut baseline_timings = Vec::new();
    let prompt = crate::bench::build_prompt(512);

    for run in 1..=config.runs {
        progress.log(&format!("[baseline] run {}/{}", run, config.runs));
        match baseline_handle.complete(&prompt, config.gen_tokens).await {
            Ok(ts) => {
                baseline_timings.push(ts);
            }
            Err(e) => {
                bail!(
                    "Baseline run {} failed: {}. Cannot continue without baseline.",
                    run,
                    e
                );
            }
        }
    }

    let (baseline_mean, baseline_stddev) = compute_mean_stddev(&baseline_timings);
    progress.log(&format!(
        "Baseline TG t/s: {:.2} ± {:.2}",
        baseline_mean, baseline_stddev
    ));

    if baseline_mean == 0.0 {
        bail!("Baseline mean is 0.0 — benchmark data may be invalid.");
    }

    // Step 3: Build sweep matrix.
    // Drop the baseline server now so GPU memory is available for spec-type servers.
    drop(baseline_handle);
    // Brief pause to let GPU memory fully free before spawning new servers.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let sweep_matrix = build_sweep_matrix(config).context("Failed to build sweep matrix")?;
    progress.log(&format!(
        "Sweep matrix: {} configurations across {} spec-types",
        sweep_matrix.len(),
        config.spec_types.len()
    ));

    // Step 4: Execute each config with its own server.
    // Each config gets a dedicated server because ngram params and draft_max
    // are server startup flags that can't be changed mid-session.
    let mut all_entries = Vec::new();
    let mut oom_detected = false;

    for cfg in &sweep_matrix {
        if oom_detected {
            progress.log(&format!(
                "[{}] skipping due to prior OOM",
                cfg.spec_type.as_str()
            ));
            all_entries.push(SpecEntry {
                spec_type: cfg.spec_type.as_str().to_string(),
                draft_max: cfg.draft_max,
                ngram_n: cfg.ngram_n,
                ngram_m: cfg.ngram_m,
                ngram_min: cfg.ngram_min,
                ngram_max: cfg.ngram_max,
                tg_ts_mean: 0.0,
                tg_ts_stddev: 0.0,
                delta_pct: 0.0,
                acceptance_rate: None,
                status: "skipped_oom".to_string(),
                error: Some("Skipped due to OOM in earlier config".to_string()),
            });
            continue;
        }

        let mut entry = run_single_config(&binary, cfg, config, baseline_mean, progress.clone()).await;

        // Brief pause between configs to let GPU memory be freed
        // before the next server starts loading the model.
        tokio::time::sleep(Duration::from_secs(2)).await;

        if entry.status == "skipped_oom" {
            oom_detected = true;
        }

        // Compute delta vs baseline.
        if entry.tg_ts_mean > 0.0 && baseline_mean > 0.0 {
            entry.delta_pct = ((entry.tg_ts_mean - baseline_mean) / baseline_mean) * 100.0;
        }

        all_entries.push(entry);
    }

    Ok(SpecBenchResult {
        baseline_tg_ts: baseline_mean,
        baseline_tg_stddev: baseline_stddev,
        entries: all_entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that the sweep matrix produces the correct number of entries for ngram-simple.
    #[test]
    fn test_sweep_matrix_ngram_simple() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramSimple],
            draft_max_values: vec![8, 16, 32],
            ngram_n_values: vec![],
            ngram_m_values: vec![],
            ngram_min_values: vec![],
            ngram_max_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // 1 spec_type × 3 draft_max = 3
        assert_eq!(matrix.len(), 3);
    }

    /// Verifies that the sweep matrix produces correct entries for ngram-mod (includes ngram_n dimension).
    #[test]
    fn test_sweep_matrix_ngram_mod() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMod],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![],
            ngram_min_values: vec![3],
            ngram_max_values: vec![48],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // 1 spec_type × 2 n-match × 1 n-min × 1 n-max = 2
        // (draft_max is NOT swept for ngram-mod)
        assert_eq!(matrix.len(), 2);
    }

    /// Verifies that the sweep matrix produces correct entries for ngram-map-k (includes ngram_m dimension).
    #[test]
    fn test_sweep_matrix_ngram_map_k() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMapK],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![2, 4],
            ngram_min_values: vec![],
            ngram_max_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // 1 spec_type × 2 draft_max × 2 ngram_n × 2 ngram_m = 8
        assert_eq!(matrix.len(), 8);
    }

    /// Verifies that the sweep matrix correctly combines multiple spec-types.
    #[test]
    fn test_sweep_matrix_multiple_spec_types() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramSimple, SpecType::NgramMod],
            draft_max_values: vec![8, 16, 32],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![],
            ngram_min_values: vec![3],
            ngram_max_values: vec![48],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // NgramSimple: 1 × 3 draft_max = 3
        // NgramMod: 1 × 2 n-match × 1 n-min × 1 n-max = 2 (no draft_max sweep)
        // Total: 5
        assert_eq!(matrix.len(), 5);
    }

    /// Verifies that build_sweep_matrix returns an error when ngram_n_values is empty but required.
    #[test]
    fn test_sweep_matrix_requires_ngram_n() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMod],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![],
            ngram_m_values: vec![],
            ngram_min_values: vec![3],
            ngram_max_values: vec![48],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let result = build_sweep_matrix(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ngram_n_values is required"));
    }

    /// Verifies that build_sweep_matrix returns an error when ngram_m_values is empty but required.
    #[test]
    fn test_sweep_matrix_requires_ngram_m() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMapK],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![],
            ngram_min_values: vec![],
            ngram_max_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let result = build_sweep_matrix(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ngram_m_values is required"));
    }

    /// Verifies that compute_mean_stddev returns correct values for a known set.
    #[test]
    fn test_compute_mean_stddev_basic() {
        let values = vec![100.0, 102.0, 98.0];
        let (mean, stddev) = compute_mean_stddev(&values);
        assert!((mean - 100.0).abs() < 0.01);
        // population stddev of [100, 102, 98] = sqrt(((0)^2 + (2)^2 + (-2)^2) / 3) = sqrt(8/3) ≈ 1.633
        assert!((stddev - 1.633).abs() < 0.01);
    }

    /// Verifies that compute_mean_stddev returns (0.0, 0.0) for an empty slice.
    #[test]
    fn test_compute_mean_stddev_empty() {
        let values: Vec<f64> = vec![];
        let (mean, stddev) = compute_mean_stddev(&values);
        assert_eq!(mean, 0.0);
        assert_eq!(stddev, 0.0);
    }

    /// Verifies that compute_mean_stddev returns stddev of 0.0 for a single value.
    #[test]
    fn test_compute_mean_stddev_single() {
        let values = vec![42.5];
        let (mean, stddev) = compute_mean_stddev(&values);
        assert!((mean - 42.5).abs() < 0.01);
        assert_eq!(stddev, 0.0);
    }

    /// Verifies that SpecType::as_str() returns correct string values.
    #[test]
    fn test_spec_type_as_str() {
        assert_eq!(SpecType::NgramSimple.as_str(), "ngram-simple");
        assert_eq!(SpecType::NgramMod.as_str(), "ngram-mod");
        assert_eq!(SpecType::NgramMapK.as_str(), "ngram-map-k");
        assert_eq!(SpecType::NgramMapK4v.as_str(), "ngram-map-k4v");
    }

    /// Verifies that the sweep matrix produces 3D n-gram-mod entries
    /// (draft_max × n-match × n-min × n-max).
    #[test]
    fn test_sweep_matrix_ngram_mod_3d() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMod],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5, 8],
            ngram_m_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
            ngram_min_values: vec![3, 5],
            ngram_max_values: vec![48, 64],
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // 1 spec_type × 3 n-match × 2 n-min × 2 n-max = 12
        // (draft_max is NOT swept for ngram-mod — controlled by n-min/n-max)
        assert_eq!(matrix.len(), 12);

        // Verify the first entry has all fields set.
        let first = &matrix[0];
        assert_eq!(first.spec_type, SpecType::NgramMod);
        assert_eq!(first.draft_max, 8);
        assert_eq!(first.ngram_n, Some(3));
        assert_eq!(first.ngram_min, Some(3));
        assert_eq!(first.ngram_max, Some(48));
    }

    /// Verifies that build_sweep_matrix returns an error when nmin/nmax values
    /// are empty but required for n-gram-mod.
    #[test]
    fn test_sweep_matrix_ngram_mod_requires_min_max() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMod],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
            ngram_min_values: vec![],
            ngram_max_values: vec![],
        };

        let result = build_sweep_matrix(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ngram_min_values") || err_msg.contains("ngram_max_values"));
    }

    /// Verifies that SpecType::spec_ngram_flags() returns correct type-specific
    /// flag names (llama.cpp PR #22397).
    #[test]
    fn test_spec_type_ngram_flags() {
        let (sn, sm, mh) = SpecType::NgramSimple.spec_ngram_flags();
        assert_eq!(sn, "--spec-ngram-simple-size-n");
        assert_eq!(sm, "--spec-ngram-simple-size-m");
        assert_eq!(mh, "--spec-ngram-simple-min-hits");

        let (sn, sm, mh) = SpecType::NgramMod.spec_ngram_flags();
        assert_eq!(sn, "--spec-ngram-mod-n-match");
        assert_eq!(sm, "");
        assert_eq!(mh, "");

        let (sn, sm, mh) = SpecType::NgramMapK.spec_ngram_flags();
        assert_eq!(sn, "--spec-ngram-map-k-size-n");
        assert_eq!(sm, "--spec-ngram-map-k-size-m");
        assert_eq!(mh, "--spec-ngram-map-k-min-hits");

        let (sn, sm, mh) = SpecType::NgramMapK4v.spec_ngram_flags();
        assert_eq!(sn, "--spec-ngram-map-k4v-size-n");
        assert_eq!(sm, "--spec-ngram-map-k4v-size-m");
        assert_eq!(mh, "--spec-ngram-map-k4v-min-hits");
    }
}
