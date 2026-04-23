//! Bench module for measuring LLM performance metrics.
//!
//! This module provides types and functions for benchmarking LLM inference:
//! - Prompt processing speed (PP tokens/s)
//! - Token generation speed (TG tokens/s)
//! - Time to first token (TTFT)
//! - Total request latency

pub mod display;
pub mod llama_bench;
pub mod llama_cli_spec;
pub mod measure;
pub mod runner;

/// Configuration for benchmark runs
#[derive(Debug, Clone, serde::Serialize)]
pub struct BenchConfig {
    /// Prompt token counts to test (default: [512])
    pub pp_sizes: Vec<u32>,
    /// Generation lengths to test (default: [128])
    pub tg_sizes: Vec<u32>,
    /// Measurement iterations (default: 3)
    pub runs: u32,
    /// Warmup iterations (default: 1)
    pub warmup: u32,
    /// Optional context size override
    pub ctx_override: Option<u32>,
    /// Logical batch size sweep (maps to llama-bench `-b`). Empty = use default.
    #[serde(default)]
    pub batch_sizes: Vec<u32>,
    /// Physical micro-batch size sweep (maps to llama-bench `-ub`). Empty = default.
    #[serde(default)]
    pub ubatch_sizes: Vec<u32>,
    /// KV cache type applied to both `-ctk` and `-ctv` (matched pair).
    #[serde(default)]
    pub kv_cache_type: Option<String>,
    /// Depth sweep (maps to llama-bench `-d`). Pre-fills tokens into KV cache
    /// before timing the measured test. Essential when evaluating KV-quant impact.
    #[serde(default)]
    pub depth: Vec<u32>,
    /// Flash attention toggle (maps to `-fa`). None = llama-bench default.
    #[serde(default)]
    pub flash_attn: Option<bool>,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            pp_sizes: vec![512],
            tg_sizes: vec![128],
            runs: 3,
            warmup: 1,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        }
    }
}

/// A single request measurement with computed metrics
#[derive(Debug, Clone)]
pub struct RequestMeasurement {
    /// Number of tokens in the prompt
    pub prompt_tokens: u32,
    /// Number of tokens generated
    pub generated_tokens: u32,
    /// Time to first token in milliseconds
    pub ttft_ms: f64,
    /// Total request time in milliseconds
    pub total_ms: f64,
    /// Prompt processing speed in tokens/second
    pub pp_tokens_per_sec: f64,
    /// Token generation speed in tokens/second
    pub tg_tokens_per_sec: f64,
}

/// Summary statistics for a test configuration
///
/// The `config_*` fields capture per-run knobs pulled from llama-bench's
/// JSON output. They're optional because llama-bench's old output (or our own
/// in-process runner) may not surface them, and because we don't want to
/// invalidate history rows written before these fields existed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BenchSummary {
    /// Test name (e.g., "pp512/tg128")
    pub test_name: String,
    /// Prompt tokens used
    pub prompt_tokens: u32,
    /// Generated tokens
    pub gen_tokens: u32,
    /// Mean prompt processing speed (tokens/s)
    pub pp_mean: f64,
    /// Stddev of prompt processing speed
    pub pp_stddev: f64,
    /// Mean token generation speed (tokens/s)
    pub tg_mean: f64,
    /// Stddev of token generation speed
    pub tg_stddev: f64,
    /// Mean time to first token (ms)
    pub ttft_mean: f64,
    /// Stddev of TTFT
    pub ttft_stddev: f64,
    /// Mean total latency (ms)
    pub total_mean: f64,
    /// Stddev of total latency
    pub total_stddev: f64,
    /// Pre-filled depth (`-d`). None for old rows or non-llama-bench runs.
    #[serde(default)]
    pub n_depth: Option<u32>,
    /// Logical batch (`-b`) for this specific run.
    #[serde(default)]
    pub n_batch: Option<u32>,
    /// Physical micro-batch (`-ub`) for this specific run.
    #[serde(default)]
    pub n_ubatch: Option<u32>,
    /// K cache quant for this run (e.g. "f16", "q8_0", "q4_0").
    #[serde(default)]
    pub type_k: Option<String>,
    /// V cache quant for this run.
    #[serde(default)]
    pub type_v: Option<String>,
    /// Flash attention on/off for this run.
    #[serde(default)]
    pub flash_attn: Option<bool>,
    /// CPU thread count used for this run.
    #[serde(default)]
    pub n_threads: Option<u32>,
    /// GPU layers loaded for this run.
    #[serde(default)]
    pub n_gpu_layers: Option<i32>,
}

/// Model metadata for display
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelInfo {
    /// Server config name
    pub name: String,
    /// Model identifier (e.g., "bartowski/Qwen2.5-Coder-7B-GGUF")
    pub model_id: Option<String>,
    /// Quantization (e.g., "Q4_K_M")
    pub quant: Option<String>,
    /// Backend config name
    pub backend: String,
    /// GPU type (e.g., "CUDA", "Vulkan", "CPU")
    pub gpu_type: String,
    /// Context length
    pub context_length: Option<u32>,
    /// GPU layers info
    pub gpu_layers: Option<String>,
}

/// Complete benchmark report
#[derive(Debug, Clone, serde::Serialize)]
pub struct BenchReport {
    /// Model metadata
    pub model_info: ModelInfo,
    /// Benchmark configuration
    pub config: BenchConfig,
    /// All test summaries
    pub summaries: Vec<BenchSummary>,
    /// Model load time in milliseconds
    pub load_time_ms: f64,
    /// VRAM info (if available)
    pub vram: Option<crate::gpu::VramInfo>,
}

/// Compute summary statistics (mean and population stddev) from a set of measurements.
///
/// # Parameters
/// - `test_name`: label for this test case, e.g. `"pp512/tg128"`.
/// - `prompt_tokens`: number of prompt tokens used in the test (PP size).
/// - `gen_tokens`: target number of generated tokens (TG size).
/// - `measurements`: slice of [`RequestMeasurement`] values to aggregate; may be empty.
///
/// # Returns
/// A [`BenchSummary`] containing mean and stddev for PP speed, TG speed, TTFT, and total
/// latency.  If `measurements` is empty all fields are `0.0`.  If it contains exactly one
/// entry stddev is `0.0`.
pub fn compute_summary(
    test_name: &str,
    prompt_tokens: u32,
    gen_tokens: u32,
    measurements: &[RequestMeasurement],
) -> BenchSummary {
    let count = measurements.len();

    if count == 0 {
        return BenchSummary {
            test_name: test_name.to_string(),
            prompt_tokens,
            gen_tokens,
            pp_mean: 0.0,
            pp_stddev: 0.0,
            tg_mean: 0.0,
            tg_stddev: 0.0,
            ttft_mean: 0.0,
            ttft_stddev: 0.0,
            total_mean: 0.0,
            total_stddev: 0.0,
            n_depth: None,
            n_batch: None,
            n_ubatch: None,
            type_k: None,
            type_v: None,
            flash_attn: None,
            n_threads: None,
            n_gpu_layers: None,
        };
    }

    // Extract metric arrays
    let pp_values: Vec<f64> = measurements.iter().map(|m| m.pp_tokens_per_sec).collect();
    let tg_values: Vec<f64> = measurements.iter().map(|m| m.tg_tokens_per_sec).collect();
    let ttft_values: Vec<f64> = measurements.iter().map(|m| m.ttft_ms).collect();
    let total_values: Vec<f64> = measurements.iter().map(|m| m.total_ms).collect();

    // Compute means
    let pp_mean = pp_values.iter().sum::<f64>() / count as f64;
    let tg_mean = tg_values.iter().sum::<f64>() / count as f64;
    let ttft_mean = ttft_values.iter().sum::<f64>() / count as f64;
    let total_mean = total_values.iter().sum::<f64>() / count as f64;

    // Compute stddev (population)
    let pp_var = if count == 1 {
        0.0
    } else {
        let diff_sum: f64 = pp_values.iter().map(|x| (x - pp_mean).powi(2)).sum();
        diff_sum / count as f64
    };
    let pp_stddev = pp_var.sqrt();

    let tg_var = if count == 1 {
        0.0
    } else {
        let diff_sum: f64 = tg_values.iter().map(|x| (x - tg_mean).powi(2)).sum();
        diff_sum / count as f64
    };
    let tg_stddev = tg_var.sqrt();

    let ttft_var = if count == 1 {
        0.0
    } else {
        let diff_sum: f64 = ttft_values.iter().map(|x| (x - ttft_mean).powi(2)).sum();
        diff_sum / count as f64
    };
    let ttft_stddev = ttft_var.sqrt();

    let total_var = if count == 1 {
        0.0
    } else {
        let diff_sum: f64 = total_values.iter().map(|x| (x - total_mean).powi(2)).sum();
        diff_sum / count as f64
    };
    let total_stddev = total_var.sqrt();

    BenchSummary {
        test_name: test_name.to_string(),
        prompt_tokens,
        gen_tokens,
        pp_mean,
        pp_stddev,
        tg_mean,
        tg_stddev,
        ttft_mean,
        ttft_stddev,
        total_mean,
        total_stddev,
        n_depth: None,
        n_batch: None,
        n_ubatch: None,
        type_k: None,
        type_v: None,
        flash_attn: None,
        n_threads: None,
        n_gpu_layers: None,
    }
}

/// Build a user message string of approximately `target_tokens` tokens.
///
/// Uses the ~4 chars/token heuristic for English text with common BPE tokenizers.
/// The base sentence ("The quick brown fox jumps over the lazy dog. ") is repeated until the
/// character count reaches `target_tokens * 4`, then truncated to that exact length.
///
/// The result is approximate and intentionally so — it provides a consistent, reproducible
/// prompt for relative performance comparisons without requiring a real tokenizer.
///
/// # Parameters
/// - `target_tokens`: desired approximate token count for the returned string.
///
/// # Returns
/// A plain text string suitable for use as the `content` of a user message.
pub fn build_prompt(target_tokens: u32) -> String {
    let repeat_text = "The quick brown fox jumps over the lazy dog. ";
    let chars_per_token = 4.0;

    let target_chars = (target_tokens as f64 * chars_per_token) as usize;
    let repeat_count = (target_chars / repeat_text.len()) + 1;

    repeat_text
        .repeat(repeat_count)
        .chars()
        .take(target_chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::display::format_stat;

    /// Verifies that `BenchConfig::default()` has the expected field values.
    #[test]
    fn test_bench_config_default() {
        let config = BenchConfig::default();
        assert_eq!(config.pp_sizes, vec![512]);
        assert_eq!(config.tg_sizes, vec![128]);
        assert_eq!(config.runs, 3);
        assert_eq!(config.warmup, 1);
        assert_eq!(config.ctx_override, None);
    }

    /// Verifies that `compute_summary` correctly computes mean and population stddev from three measurements.
    #[test]
    fn test_compute_summary_basic() {
        let measurements = vec![
            RequestMeasurement {
                prompt_tokens: 512,
                generated_tokens: 128,
                ttft_ms: 100.0,
                total_ms: 500.0,
                pp_tokens_per_sec: 5120.0,
                tg_tokens_per_sec: 1000.0,
            },
            RequestMeasurement {
                prompt_tokens: 512,
                generated_tokens: 128,
                ttft_ms: 102.0,
                total_ms: 502.0,
                pp_tokens_per_sec: 5020.0,
                tg_tokens_per_sec: 990.0,
            },
            RequestMeasurement {
                prompt_tokens: 512,
                generated_tokens: 128,
                ttft_ms: 98.0,
                total_ms: 498.0,
                pp_tokens_per_sec: 5220.0,
                tg_tokens_per_sec: 1010.0,
            },
        ];

        let summary = compute_summary("pp512/tg128", 512, 128, &measurements);

        // Mean should be approximately (5120 + 5020 + 5220) / 3 = 5120
        assert!((summary.pp_mean - 5120.0).abs() < 1.0);
        assert!((summary.tg_mean - 1000.0).abs() < 1.0);
        assert!((summary.ttft_mean - 100.0).abs() < 1.0);
        assert!((summary.total_mean - 500.0).abs() < 1.0);

        // Stddev calculation: sqrt(sum((x-mean)^2)/n)
        // For pp: values are [5120, 5020, 5220], mean = 5120
        // diff = [0, -100, 100], diff^2 = [0, 10000, 10000], sum = 20000
        // variance = 20000 / 3 = 6666.67, stddev = sqrt(6666.67) ≈ 81.65
        assert!((summary.pp_stddev - 81.65).abs() < 1.0);
    }

    /// Verifies that `compute_summary` returns stddev of 0.0 when given a single measurement.
    #[test]
    fn test_compute_summary_single_measurement() {
        let measurements = vec![RequestMeasurement {
            prompt_tokens: 512,
            generated_tokens: 128,
            ttft_ms: 100.0,
            total_ms: 500.0,
            pp_tokens_per_sec: 5120.0,
            tg_tokens_per_sec: 1000.0,
        }];

        let summary = compute_summary("pp512/tg128", 512, 128, &measurements);

        // With only one measurement, stddev should be 0.0
        assert_eq!(summary.pp_stddev, 0.0);
        assert_eq!(summary.tg_stddev, 0.0);
        assert_eq!(summary.ttft_stddev, 0.0);
        assert_eq!(summary.total_stddev, 0.0);
    }

    /// Verifies that `compute_summary` returns all-zero fields when given an empty slice.
    #[test]
    fn test_compute_summary_empty() {
        let summary = compute_summary("pp512/tg128", 512, 128, &[]);

        assert_eq!(summary.pp_mean, 0.0);
        assert_eq!(summary.tg_mean, 0.0);
        assert_eq!(summary.ttft_mean, 0.0);
        assert_eq!(summary.total_mean, 0.0);
        assert_eq!(summary.pp_stddev, 0.0);
        assert_eq!(summary.tg_stddev, 0.0);
        assert_eq!(summary.ttft_stddev, 0.0);
        assert_eq!(summary.total_stddev, 0.0);
    }

    /// Verifies that `build_prompt(512)` produces a string within the expected character-count range.
    #[test]
    fn test_build_prompt_approximate_length() {
        let prompt = build_prompt(512);
        // Should be roughly 512 * 4 = 2048 chars, allow some tolerance
        assert!(prompt.len() >= 1500 && prompt.len() <= 2500);
    }

    /// Verifies that a larger `target_tokens` value produces a proportionally longer prompt string.
    #[test]
    fn test_build_prompt_scales() {
        let prompt_512 = build_prompt(512);
        let prompt_1024 = build_prompt(1024);
        assert!(prompt_1024.len() > prompt_512.len());
    }

    /// Verifies that `format_stat` returns "mean ± stddev" when stddev is non-zero.
    #[test]
    fn test_format_stat_with_stddev() {
        let result = format_stat(4821.3, 42.1);
        assert_eq!(result, "4821.3 ± 42.1");
    }

    /// Verifies that `format_stat` returns only the mean when stddev is 0.0.
    #[test]
    fn test_format_stat_zero_stddev() {
        let result = format_stat(4821.3, 0.0);
        assert_eq!(result, "4821.3");
    }
}
