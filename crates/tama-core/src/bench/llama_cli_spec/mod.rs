//! llama-cli speculative decoding benchmark module.
//!
//! Wraps the `llama-cli` binary from llama.cpp's tools/ directory to run
//! benchmarks with speculative decoding flags (`--spec-type`, `--draft-max`,
//! etc.). Unlike `llama-bench`, `llama-cli` supports these experimental
//! inference acceleration techniques.
//!
//! Split into:
//! - [`args`] — CLI-argument construction from [`SpecBenchConfig`].
//! - [`discovery`] — binary lookup (pure filesystem logic).
//! - [`parse`] — timing extraction from llama-cli stderr output.
//!
//! This module's `mod.rs` keeps the public types ([`SpecType`],
//! [`SpecBenchConfig`], [`SpecEntry`], [`SpecBenchResult`]) and the async
//! orchestrator [`run_spec_bench`].

mod args;
mod discovery;
mod parse;

pub use discovery::find_llama_cli;

use crate::backends::ProgressSink;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

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
    pub model_path: PathBuf,
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

/// A single sweep configuration to test.
#[derive(Debug, Clone)]
struct SweepConfig {
    spec_type: SpecType,
    draft_max: u32,
    ngram_n: Option<u32>,
    ngram_m: Option<u32>,
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

    if needs_n && config.ngram_n_values.is_empty() {
        bail!("ngram_n_values is required when testing ngram-mod or ngram-map-* types");
    }
    if needs_m && config.ngram_m_values.is_empty() {
        bail!("ngram_m_values is required when testing ngram-map-k or ngram-map-k4v types");
    }

    let mut matrix = Vec::new();

    for &st in spec_types {
        for &dm in &config.draft_max_values {
            match st {
                SpecType::NgramSimple => {
                    matrix.push(SweepConfig {
                        spec_type: st,
                        draft_max: dm,
                        ngram_n: None,
                        ngram_m: None,
                    });
                }
                SpecType::NgramMod => {
                    for &nn in &config.ngram_n_values {
                        matrix.push(SweepConfig {
                            spec_type: st,
                            draft_max: dm,
                            ngram_n: Some(nn),
                            ngram_m: None,
                        });
                    }
                }
                SpecType::NgramMapK | SpecType::NgramMapK4v => {
                    for &nn in &config.ngram_n_values {
                        for &nm in &config.ngram_m_values {
                            matrix.push(SweepConfig {
                                spec_type: st,
                                draft_max: dm,
                                ngram_n: Some(nn),
                                ngram_m: Some(nm),
                            });
                        }
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

/// Outcome of a single llama-cli execution.
#[derive(Debug)]
enum RunOutcome {
    /// Success with parsed timing and captured stderr.
    Success(f64, String),
    /// Process exited with error. Contains (exit_code_display, stderr).
    Failed(String, String),
}

/// Run a single llama-cli command and return the outcome.
///
/// Does NOT bail on non-success exit — returns `Failed` with stderr so the
/// caller can classify the error (OOM vs other failures).
async fn run_llama_cli_once(binary: &PathBuf, args: &[String]) -> Result<RunOutcome> {
    use std::time::Duration;


    // Parse timeout from env, default to 5 minutes
    let timeout_secs = std::env::var("LLAMA_CLI_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300);

    let child = Command::new(binary)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Failed to execute llama-cli")?;

    match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();


            if !output.status.success() {
                return Ok(RunOutcome::Failed(format!("{:?}", output.status), stderr));
            }

            // Try parsing stderr first, then stdout.
            let timing = parse::parse_timing(&stderr).or_else(|_| parse::parse_timing(&stdout))?;


            Ok(RunOutcome::Success(timing, stderr))
        }
        Ok(Err(e)) => anyhow::bail!("wait_with_output failed: {e}"),
        Err(_) => anyhow::bail!(
            "llama-cli timed out after {timeout_secs}s. Args: {:?}",
            args
        ),
    }
}

/// Execute a benchmark run with retry logic and OOM detection.
async fn execute_config_run(
    binary: &PathBuf,
    sweep_cfg: &SweepConfig,
    bench_cfg: &SpecBenchConfig,
    progress: &dyn ProgressSink,
) -> SpecEntry {
    let args = args::build_args(
        bench_cfg,
        sweep_cfg.spec_type,
        sweep_cfg.draft_max,
        sweep_cfg.ngram_n,
        sweep_cfg.ngram_m,
    );
    let label = format!(
        "{} draft_max={} n={:?} m={:?}",
        sweep_cfg.spec_type.as_str(),
        sweep_cfg.draft_max,
        sweep_cfg.ngram_n,
        sweep_cfg.ngram_m,
    );

    let mut timings = Vec::new();

    for run in 1..=bench_cfg.runs {
        progress.log(&format!("[{}] run {}/{}", label, run, bench_cfg.runs));

        let result = match run_llama_cli_once(binary, &args).await {
            Ok(o) => o,
            Err(e) => RunOutcome::Failed("spawn failed".to_string(), e.to_string()),
        };

        match result {
            RunOutcome::Success(timing, _stderr) => {
                timings.push(timing);
            }
            RunOutcome::Failed(code, stderr) => {
                // Check for OOM in stderr
                if stderr.to_lowercase().contains("oom")
                    || stderr.to_lowercase().contains("out of memory")
                {
                    progress.log(&format!("[{}] OOM detected on run {}", label, run));
                    return SpecEntry {
                        spec_type: sweep_cfg.spec_type.as_str().to_string(),
                        draft_max: sweep_cfg.draft_max,
                        ngram_n: sweep_cfg.ngram_n,
                        ngram_m: sweep_cfg.ngram_m,
                        tg_ts_mean: 0.0,
                        tg_ts_stddev: 0.0,
                        delta_pct: 0.0,
                        status: "skipped_oom".to_string(),
                        error: Some(format!(
                            "OOM detected: {}",
                            stderr
                                .lines()
                                .find(|l| l.to_lowercase().contains("oom")
                                    || l.to_lowercase().contains("out of memory"))
                                .unwrap_or("unknown")
                        )),
                    };
                }

                // Retry once (2 total attempts)
                progress.log(&format!(
                    "[{}] run {} failed ({}): {}",
                    label,
                    run,
                    code,
                    stderr.lines().take(3).collect::<Vec<_>>().join(" ")
                ));
                let retry_result = match run_llama_cli_once(binary, &args).await {
                    Ok(o) => o,
                    Err(e) => RunOutcome::Failed("spawn failed".to_string(), e.to_string()),
                };

                match retry_result {
                    RunOutcome::Success(timing, _stderr) => {
                        timings.push(timing);
                    }
                    RunOutcome::Failed(retry_code, retry_stderr) => {
                        if retry_stderr.to_lowercase().contains("oom")
                            || retry_stderr.to_lowercase().contains("out of memory")
                        {
                            progress.log(&format!("[{}] OOM detected on retry", label));
                            return SpecEntry {
                                spec_type: sweep_cfg.spec_type.as_str().to_string(),
                                draft_max: sweep_cfg.draft_max,
                                ngram_n: sweep_cfg.ngram_n,
                                ngram_m: sweep_cfg.ngram_m,
                                tg_ts_mean: 0.0,
                                tg_ts_stddev: 0.0,
                                delta_pct: 0.0,
                                status: "skipped_oom".to_string(),
                                error: Some("OOM detected during retry".to_string()),
                            };
                        }
                        let err_msg = format!(
                            "exit={} stderr={} | retry: exit={} stderr={}",
                            code,
                            stderr.lines().take(2).collect::<Vec<_>>().join(" "),
                            retry_code,
                            retry_stderr.lines().take(2).collect::<Vec<_>>().join(" ")
                        );
                        progress.log(&format!("[{}] failed after retry: {}", label, err_msg));
                        return SpecEntry {
                            spec_type: sweep_cfg.spec_type.as_str().to_string(),
                            draft_max: sweep_cfg.draft_max,
                            ngram_n: sweep_cfg.ngram_n,
                            ngram_m: sweep_cfg.ngram_m,
                            tg_ts_mean: 0.0,
                            tg_ts_stddev: 0.0,
                            delta_pct: 0.0,
                            status: "failed".to_string(),
                            error: Some(err_msg),
                        };
                    }
                }
            }
        }
    }

    let (mean, stddev) = compute_mean_stddev(&timings);
    progress.log(&format!(
        "[{}] completed: {:.2} ± {:.2} tokens/s",
        label, mean, stddev
    ));

    SpecEntry {
        spec_type: sweep_cfg.spec_type.as_str().to_string(),
        draft_max: sweep_cfg.draft_max,
        ngram_n: sweep_cfg.ngram_n,
        ngram_m: sweep_cfg.ngram_m,
        tg_ts_mean: mean,
        tg_ts_stddev: stddev,
        delta_pct: 0.0, // will be filled in by caller
        status: "success".to_string(),
        error: None,
    }
}

/// Run a speculative decoding benchmark sweep.
///
/// # Arguments
/// - `config`: the benchmark configuration specifying model, spec types, and sweep dimensions.
/// - `binary_path`: optional override for the llama-cli binary path. If `None`, uses discovery.
/// - `progress`: progress sink for streaming status updates.
///
/// # Returns
/// A [`SpecBenchResult`] with baseline timing and one entry per sweep configuration.
pub async fn run_spec_bench(
    config: &SpecBenchConfig,
    binary_path: Option<PathBuf>,
    progress: &dyn ProgressSink,
) -> Result<SpecBenchResult> {
    // Step 1: Discover or use provided llama-cli binary
    let binary = if let Some(bp) = binary_path {
        if !bp.exists() {
            bail!("Provided llama-cli path does not exist: {}", bp.display());
        }
        bp
    } else {
        discovery::find_llama_cli(
            config
                .model_path
                .parent()
                .unwrap_or(std::path::Path::new("")),
        )
        .context("llama-cli not found. Set LLAMA_CLI_PATH or install llama.cpp from source.")?
    };

    progress.log(&format!("Using llama-cli: {}", binary.display()));
    progress.log(&format!(
        "Model: {} (gen_tokens={}, runs={})",
        config.model_path.display(),
        config.gen_tokens,
        config.runs,
    ));

    // Step 2: Run baseline (no spec-decoding flags)
    progress.log("Running baseline (no speculative decoding)...");
    let baseline_args = args::build_baseline_args(config);
    let mut baseline_timings = Vec::new();

    for run in 1..=config.runs {
        progress.log(&format!("[baseline] run {}/{}", run, config.runs));
        let outcome = match run_llama_cli_once(&binary, &baseline_args).await {
            Ok(o) => o,
            Err(e) => RunOutcome::Failed("spawn failed".to_string(), e.to_string()),
        };
        match outcome {
            RunOutcome::Success(timing, _stderr) => {
                baseline_timings.push(timing);
            }
            RunOutcome::Failed(code, stderr) => {
                progress.log(&format!(
                    "[baseline] run {} failed ({}): {}",
                    run,
                    code,
                    stderr.lines().take(2).collect::<Vec<_>>().join(" ")
                ));
                // Retry once
                let retry_outcome = match run_llama_cli_once(&binary, &baseline_args).await {
                    Ok(o) => o,
                    Err(e) => RunOutcome::Failed("spawn failed".to_string(), e.to_string()),
                };
                match retry_outcome {
                    RunOutcome::Success(timing, _stderr) => {
                        baseline_timings.push(timing);
                    }
                    RunOutcome::Failed(retry_code, retry_stderr) => {
                        bail!(
                            "Baseline failed after retry: exit={} stderr={} | retry: exit={} stderr={}. Cannot continue sweep without baseline.",
                            code,
                            stderr.lines().take(2).collect::<Vec<_>>().join(" "),
                            retry_code,
                            retry_stderr.lines().take(2).collect::<Vec<_>>().join(" ")
                        );
                    }
                }
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

    // Step 3: Build sweep matrix
    let sweep_matrix = build_sweep_matrix(config).context("Failed to build sweep matrix")?;
    progress.log(&format!(
        "Sweep matrix: {} configurations to test",
        sweep_matrix.len()
    ));

    // Step 4: Execute each config
    let mut entries = Vec::new();
    let mut oom_detected = false;

    for (i, sweep_cfg) in sweep_matrix.iter().enumerate() {
        if oom_detected {
            progress.log(&format!(
                "[{}] skipping due to prior OOM",
                sweep_cfg.spec_type.as_str()
            ));
            entries.push(SpecEntry {
                spec_type: sweep_cfg.spec_type.as_str().to_string(),
                draft_max: sweep_cfg.draft_max,
                ngram_n: sweep_cfg.ngram_n,
                ngram_m: sweep_cfg.ngram_m,
                tg_ts_mean: 0.0,
                tg_ts_stddev: 0.0,
                delta_pct: 0.0,
                status: "skipped_oom".to_string(),
                error: Some("Skipped due to OOM in earlier config".to_string()),
            });
            continue;
        }

        progress.log(&format!(
            "[{}/{}] Testing {} draft_max={} n={:?} m={:?}",
            i + 1,
            sweep_matrix.len(),
            sweep_cfg.spec_type.as_str(),
            sweep_cfg.draft_max,
            sweep_cfg.ngram_n,
            sweep_cfg.ngram_m,
        ));

        let entry = execute_config_run(&binary, sweep_cfg, config, progress).await;

        if entry.status == "skipped_oom" {
            oom_detected = true;
        }

        // Compute delta vs baseline
        let delta_pct = if entry.tg_ts_mean > 0.0 && baseline_mean > 0.0 {
            ((entry.tg_ts_mean - baseline_mean) / baseline_mean) * 100.0
        } else {
            0.0
        };
        let mut entry = entry;
        entry.delta_pct = delta_pct;

        entries.push(entry);
    }

    Ok(SpecBenchResult {
        baseline_tg_ts: baseline_mean,
        baseline_tg_stddev: baseline_stddev,
        entries,
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
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // 1 spec_type × 2 draft_max × 2 ngram_n = 4
        assert_eq!(matrix.len(), 4);
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
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // NgramSimple: 1 × 3 = 3
        // NgramMod: 1 × 3 × 2 = 6
        // Total: 9
        assert_eq!(matrix.len(), 9);
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
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let result = build_sweep_matrix(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ngram_n_values is required"));
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
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let result = build_sweep_matrix(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ngram_m_values is required"));
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
}
