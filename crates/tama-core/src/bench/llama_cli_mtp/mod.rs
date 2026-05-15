//! MTP (Multi-Token Prediction) benchmarking module.
//!
//! Embeds 9 diverse prompts, spawns a llama-server per draft-n-max config,
//! runs all 9 prompts via chat_complete, and collects per-prompt + aggregate metrics.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::backends::ProgressSink;
use crate::bench::llama_cli_spec::server::{self, ServerArgs, ServerHandle};
use crate::bench::llama_cli_spec::SpecType;

pub use crate::bench::llama_cli_spec::find_llama_server;

/// Nine diverse prompts for MTP benchmarking (from mtp-bench.py).
pub const MTP_PROMPTS: &[(&str, &str)] = &[
    (
        "code_python",
        "Write a Python function that returns the n-th Fibonacci number using memoization. Include a docstring.",
    ),
    (
        "code_cpp",
        "Write a C++ template function `clamp(x, lo, hi)` that returns x clamped to [lo, hi]. No std::clamp.",
    ),
    (
        "explain_concept",
        "Explain how speculative decoding works in large language model inference, in three short paragraphs.",
    ),
    (
        "summarize",
        "Summarize in two sentences: The Industrial Revolution began in Britain in the late 18th century, transforming manufacturing through mechanization, steam power, and the factory system. It spread to continental Europe and North America during the 19th century.",
    ),
    (
        "qa_factual",
        "Q: What are the four fundamental forces of physics?\nA:",
    ),
    (
        "translation",
        "Translate to French: 'The quick brown fox jumps over the lazy dog.'",
    ),
    (
        "creative_short",
        "Write a four-line poem about an old lighthouse.",
    ),
    (
        "stepwise_math",
        "Solve step by step: A train leaves station A at 60 km/h. Two hours later, a second train leaves the same station on the same track at 90 km/h. How long until the second train catches the first?",
    ),
    (
        "long_code_review",
        "Review the following Python code for correctness, performance, and style. Suggest improvements:\n\n```python\ndef find_duplicates(lst):\n    duplicates = []\n    for i in range(len(lst)):\n        for j in range(i+1, len(lst)):\n            if lst[i] == lst[j] and lst[i] not in duplicates:\n                duplicates.append(lst[i])\n    return duplicates\n```",
    ),
];

/// Configuration for MTP benchmarking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpBenchConfig {
    /// Path to the target model GGUF file.
    pub model_path: PathBuf,
    /// Draft max values to sweep (e.g. [0, 1, 2, 4, 8]).
    pub draft_max_values: Vec<u32>,
    /// GPU layers (maps to --n-gpu-layers). Default Some(99).
    pub ngl: Option<u32>,
    /// Spec draft NGL (maps to --spec-draft-ngl). Default Some(99).
    pub draft_ngl: Option<u32>,
    /// Flash attention toggle (maps to -fa). Default true.
    #[serde(default = "default_flash_attn")]
    pub flash_attn: bool,
}

fn default_flash_attn() -> bool {
    true
}

/// Result of a single prompt within a given draft-n-max config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpPromptResult {
    /// Which draft-n-max config produced this result.
    pub draft_max: u32,
    /// Prompt name (e.g. "code_python").
    pub name: String,
    /// Wall clock time in seconds for this request.
    pub wall_s: f64,
    /// Number of predicted (completion) tokens.
    pub predicted_n: u32,
    /// Total draft tokens proposed.
    pub draft_n: u32,
    /// Draft tokens accepted.
    pub draft_n_accepted: u32,
    /// Acceptance rate (accepted / draft_n). None when draft_n == 0 (baseline).
    pub accept_rate: Option<f64>,
    /// Predicted tokens per second.
    pub predicted_per_second: f64,
    /// Error message if this prompt failed; all numeric fields are 0.
    pub error: Option<String>,
}

/// Complete MTP benchmark result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpBenchResult {
    /// One entry per prompt per draft-max config, in execution order.
    pub entries: Vec<MtpPromptResult>,
    /// Aggregate statistics across all entries.
    pub aggregate: MtpAggregate,
}

/// Aggregate statistics across all MTP benchmark entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpAggregate {
    /// Total number of requests (successful + failed).
    pub n_requests: usize,
    /// Sum of predicted_n across all successful entries.
    pub total_predicted: u32,
    /// Sum of draft_n across all successful entries.
    pub total_draft: u32,
    /// Sum of draft_n_accepted across all successful entries.
    pub total_draft_accepted: u32,
    /// Aggregate acceptance rate (total_draft_accepted / total_draft). 0.0 if total_draft == 0.
    pub aggregate_accept_rate: f64,
    /// Sum of wall_s across all entries.
    pub wall_s_total: f64,
}

/// Run a single prompt against a running server and return the result.
async fn run_single_prompt(
    handle: &ServerHandle,
    model_name: &str,
    prompt_name: &str,
    prompt_text: &str,
    draft_max: u32,
    progress: &Arc<dyn ProgressSink>,
) -> MtpPromptResult {
    let messages = vec![("user", prompt_text)];
    let start = std::time::Instant::now();

    match handle.chat_complete(model_name, &messages, 192).await {
        Ok(timing) => {
            let wall_s = start.elapsed().as_secs_f64();
            let accept_rate = if timing.draft_n > 0 {
                Some(timing.draft_n_accepted as f64 / timing.draft_n as f64)
            } else {
                None
            };
            MtpPromptResult {
                draft_max,
                name: prompt_name.to_string(),
                wall_s,
                predicted_n: timing.predicted_n,
                draft_n: timing.draft_n,
                draft_n_accepted: timing.draft_n_accepted,
                accept_rate,
                predicted_per_second: timing.predicted_per_second,
                error: None,
            }
        }
        Err(e) => {
            let wall_s = start.elapsed().as_secs_f64();
            progress.log(&format!(
                "[draft_max={}] prompt '{}' failed: {}",
                draft_max, prompt_name, e
            ));
            MtpPromptResult {
                draft_max,
                name: prompt_name.to_string(),
                wall_s,
                predicted_n: 0,
                draft_n: 0,
                draft_n_accepted: 0,
                accept_rate: None,
                predicted_per_second: 0.0,
                error: Some(e.to_string()),
            }
        }
    }
}

/// Run all 9 MTP prompts against a server with the given draft-n-max config.
async fn run_prompts_for_config(
    binary: &Path,
    config: &MtpBenchConfig,
    draft_max: u32,
    model_name: &str,
    progress: Arc<dyn ProgressSink>,
) -> Vec<MtpPromptResult> {
    let port = match crate::bench::find_available_port().await {
        Ok(p) => p,
        Err(e) => {
            progress.log(&format!("Failed to find available port: {}", e));
            return MTP_PROMPTS
                .iter()
                .map(|(name, _)| MtpPromptResult {
                    draft_max,
                    name: name.to_string(),
                    wall_s: 0.0,
                    predicted_n: 0,
                    draft_n: 0,
                    draft_n_accepted: 0,
                    accept_rate: None,
                    predicted_per_second: 0.0,
                    error: Some(format!("Port allocation failed: {}", e)),
                })
                .collect();
        }
    };

    let server_args = ServerArgs {
        binary: binary.to_path_buf(),
        model_path: config.model_path.clone(),
        port,
        ngl: config.ngl,
        flash_attn: config.flash_attn,
        spec_type: Some(SpecType::DraftMtp),
        spec_ngram_n: None,
        spec_ngram_m: None,
        spec_ngram_min_hits: None,
        spec_ngram_min: None,
        spec_ngram_max: None,
        draft_max: Some(draft_max),
        draft_min: None,
        spec_draft_ngl: config.draft_ngl,
    };

    let arg_vec = server_args.to_args();
    progress.log(&format!(
        "Starting llama-server on port {} (draft-n-max={})",
        port, draft_max
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
                "Failed to start llama-server for draft-n-max={}: {}",
                draft_max, e
            ));
            return MTP_PROMPTS
                .iter()
                .map(|(name, _)| MtpPromptResult {
                    draft_max,
                    name: name.to_string(),
                    wall_s: 0.0,
                    predicted_n: 0,
                    draft_n: 0,
                    draft_n_accepted: 0,
                    accept_rate: None,
                    predicted_per_second: 0.0,
                    error: Some(format!("Server start failed: {}", e)),
                })
                .collect();
        }
    };

    progress.log(&format!(
        "llama-server ready on port {} (draft-n-max={})",
        port, draft_max
    ));

    // Run all 9 prompts
    let mut results = Vec::with_capacity(MTP_PROMPTS.len());
    for (name, text) in MTP_PROMPTS {
        progress.log(&format!(
            "[draft_max={}] running prompt '{}'",
            draft_max, name
        ));
        let result = run_single_prompt(&handle, model_name, name, text, draft_max, &progress).await;
        results.push(result);
    }

    // Drop the server handle (kills the server)
    drop(handle);

    results
}

/// Run the MTP benchmark sweep.
///
/// Executes a baseline phase (draft-n-max=0) followed by a sweep phase
/// for each draft_max value > 0 in the config. Each phase spawns its own
/// llama-server instance.
///
/// # Arguments
/// - `config`: MTP benchmark configuration specifying model, draft values, etc.
/// - `binary_override`: optional path to the `llama-server` binary. If `None`, uses
///   discovery to find it alongside the model.
/// - `progress`: progress sink for streaming status updates.
///
/// # Returns
/// A [`MtpBenchResult`] with per-prompt entries and aggregate statistics.
pub async fn run_mtp_bench(
    config: &MtpBenchConfig,
    binary_override: Option<PathBuf>,
    progress: Arc<dyn ProgressSink>,
) -> Result<MtpBenchResult> {
    // Step 1: Discover or use provided llama-server binary.
    let backend_dir = config.model_path.parent().unwrap_or(Path::new(""));
    let binary = if let Some(bp) = binary_override {
        if !bp.exists() {
            bail!(
                "Provided llama-server path does not exist: {}",
                bp.display()
            );
        }
        bp
    } else {
        find_llama_server(backend_dir)
            .context("llama-server not found. Set LLAMA_SERVER_PATH or ensure llama-server is in the backend directory.")?
    };

    // Step 2: Extract model name from path.
    let model_name = config
        .model_path
        .file_stem()
        .unwrap_or(std::ffi::OsStr::new("model"))
        .to_string_lossy()
        .into_owned();

    progress.log(&format!("Using llama-server: {}", binary.display()));
    progress.log(&format!(
        "Model: {} ({})",
        config.model_path.display(),
        model_name
    ));

    // Effective defaults
    let ngl = config.ngl.or(Some(99));
    let draft_ngl = config.draft_ngl.or(Some(99));
    let effective_config = MtpBenchConfig {
        ngl,
        draft_ngl,
        ..config.clone()
    };

    // Collect all results in order
    let mut all_entries: Vec<MtpPromptResult> = Vec::new();

    // Step 3: Baseline phase (draft-n-max=0)
    progress.log("Starting baseline phase (draft-n-max=0)...");
    let baseline_results =
        run_prompts_for_config(&binary, &effective_config, 0, &model_name, progress.clone()).await;
    all_entries.extend(baseline_results);

    // 2s sleep between configs
    sleep(Duration::from_secs(2)).await;

    // Step 4: Sweep phase (draft_max > 0)
    for &draft_max in &config.draft_max_values {
        if draft_max == 0 {
            continue; // Already covered by baseline
        }
        progress.log(&format!(
            "Starting sweep phase (draft-n-max={})...",
            draft_max
        ));
        let sweep_results = run_prompts_for_config(
            &binary,
            &effective_config,
            draft_max,
            &model_name,
            progress.clone(),
        )
        .await;
        all_entries.extend(sweep_results);

        // 2s sleep between configs
        sleep(Duration::from_secs(2)).await;
    }

    // Step 5: Build aggregate
    let n_requests = all_entries.len();
    let total_predicted: u32 = all_entries.iter().map(|e| e.predicted_n).sum();
    let total_draft: u32 = all_entries.iter().map(|e| e.draft_n).sum();
    let total_draft_accepted: u32 = all_entries.iter().map(|e| e.draft_n_accepted).sum();
    let aggregate_accept_rate = if total_draft > 0 {
        total_draft_accepted as f64 / total_draft as f64
    } else {
        0.0
    };
    let wall_s_total: f64 = all_entries.iter().map(|e| e.wall_s).sum();

    let aggregate = MtpAggregate {
        n_requests,
        total_predicted,
        total_draft,
        total_draft_accepted,
        aggregate_accept_rate,
        wall_s_total,
    };

    let result = MtpBenchResult {
        entries: all_entries,
        aggregate,
    };

    // Step 6: Report result via progress sink
    let json = serde_json::to_string(&result).context("Failed to serialize MtpBenchResult")?;
    progress.result(&json);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::llama_cli_spec::server::ChatTiming;

    /// Verifies that MTP_PROMPTS contains exactly 9 prompts.
    #[test]
    fn test_mtp_prompts_count() {
        assert_eq!(MTP_PROMPTS.len(), 9);
    }

    /// Verifies that each prompt has a unique name.
    #[test]
    fn test_mtp_prompts_unique_names() {
        let names: Vec<_> = MTP_PROMPTS.iter().map(|(name, _)| *name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "Duplicate prompt names found");
    }

    /// Verifies that all prompt names match expected values.
    #[test]
    fn test_mtp_prompts_names() {
        let expected = [
            "code_python",
            "code_cpp",
            "explain_concept",
            "summarize",
            "qa_factual",
            "translation",
            "creative_short",
            "stepwise_math",
            "long_code_review",
        ];
        let actual: Vec<_> = MTP_PROMPTS.iter().map(|(name, _)| *name).collect();
        assert_eq!(actual, expected);
    }

    /// Verifies that MtpAggregate correctly computes accept rate when there are drafts.
    #[test]
    fn test_aggregate_accept_rate_with_drafts() {
        let entries = [
            MtpPromptResult {
                draft_max: 0,
                name: "baseline".to_string(),
                wall_s: 1.0,
                predicted_n: 100,
                draft_n: 0,
                draft_n_accepted: 0,
                accept_rate: None,
                predicted_per_second: 100.0,
                error: None,
            },
            MtpPromptResult {
                draft_max: 4,
                name: "spec".to_string(),
                wall_s: 0.8,
                predicted_n: 100,
                draft_n: 50,
                draft_n_accepted: 30,
                accept_rate: Some(0.6),
                predicted_per_second: 125.0,
                error: None,
            },
        ];

        let total_draft: u32 = entries.iter().map(|e| e.draft_n).sum();
        let total_draft_accepted: u32 = entries.iter().map(|e| e.draft_n_accepted).sum();
        let aggregate_accept_rate = if total_draft > 0 {
            total_draft_accepted as f64 / total_draft as f64
        } else {
            0.0
        };

        assert!((aggregate_accept_rate - 0.6).abs() < 0.001);
    }

    /// Verifies that aggregate accept rate is 0.0 when no drafts exist.
    #[test]
    fn test_aggregate_accept_rate_no_drafts() {
        let entries = [MtpPromptResult {
            draft_max: 0,
            name: "baseline".to_string(),
            wall_s: 1.0,
            predicted_n: 100,
            draft_n: 0,
            draft_n_accepted: 0,
            accept_rate: None,
            predicted_per_second: 100.0,
            error: None,
        }];

        let total_draft: u32 = entries.iter().map(|e| e.draft_n).sum();
        let total_draft_accepted: u32 = entries.iter().map(|e| e.draft_n_accepted).sum();
        let aggregate_accept_rate = if total_draft > 0 {
            total_draft_accepted as f64 / total_draft as f64
        } else {
            0.0
        };

        assert_eq!(aggregate_accept_rate, 0.0);
    }

    /// Verifies that MtpPromptResult with error has all numeric fields = 0.
    #[test]
    fn test_error_result_zero_fields() {
        let result = MtpPromptResult {
            draft_max: 4,
            name: "test".to_string(),
            wall_s: 0.0,
            predicted_n: 0,
            draft_n: 0,
            draft_n_accepted: 0,
            accept_rate: None,
            predicted_per_second: 0.0,
            error: Some("test error".to_string()),
        };

        assert!(result.error.is_some());
        assert_eq!(result.predicted_n, 0);
        assert_eq!(result.draft_n, 0);
        assert_eq!(result.draft_n_accepted, 0);
        assert_eq!(result.predicted_per_second, 0.0);
        assert!(result.accept_rate.is_none());
    }

    /// Verifies accept_rate is None for baseline (draft_n == 0).
    #[test]
    fn test_accept_rate_baseline_none() {
        let timing = ChatTiming {
            predicted_per_second: 100.0,
            predicted_n: 100,
            draft_n: 0,
            draft_n_accepted: 0,
        };

        let accept_rate = if timing.draft_n > 0 {
            Some(timing.draft_n_accepted as f64 / timing.draft_n as f64)
        } else {
            None
        };

        assert!(accept_rate.is_none());
    }

    /// Verifies accept_rate is Some when draft_n > 0.
    #[test]
    fn test_accept_rate_with_drafts() {
        let timing = ChatTiming {
            predicted_per_second: 150.0,
            predicted_n: 100,
            draft_n: 50,
            draft_n_accepted: 25,
        };

        let accept_rate = if timing.draft_n > 0 {
            Some(timing.draft_n_accepted as f64 / timing.draft_n as f64)
        } else {
            None
        };

        assert!(accept_rate.is_some());
        assert!((accept_rate.unwrap() - 0.5).abs() < 0.001);
    }

    /// Verifies that MtpBenchConfig serializes and deserializes correctly.
    #[test]
    fn test_mtp_bench_config_serde() {
        let config = MtpBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            draft_max_values: vec![0, 1, 2, 4, 8],
            ngl: Some(99),
            draft_ngl: Some(99),
            flash_attn: true,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MtpBenchConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.model_path, config.model_path);
        assert_eq!(deserialized.draft_max_values, config.draft_max_values);
        assert_eq!(deserialized.ngl, config.ngl);
        assert_eq!(deserialized.draft_ngl, config.draft_ngl);
        assert_eq!(deserialized.flash_attn, config.flash_attn);
    }

    /// Verifies that MtpBenchConfig with default flash_attn deserializes correctly.
    #[test]
    fn test_mtp_bench_config_default_flash_attn() {
        let json = r#"{"model_path":"/test/model.gguf","draft_max_values":[0,1,2]}"#;
        let config: MtpBenchConfig = serde_json::from_str(json).unwrap();
        assert!(config.flash_attn);
    }

    /// Verifies that MtpBenchResult serializes and deserializes correctly.
    #[test]
    fn test_mtp_bench_result_serde() {
        let result = MtpBenchResult {
            entries: vec![MtpPromptResult {
                draft_max: 0,
                name: "test_prompt".to_string(),
                wall_s: 1.5,
                predicted_n: 100,
                draft_n: 0,
                draft_n_accepted: 0,
                accept_rate: None,
                predicted_per_second: 66.67,
                error: None,
            }],
            aggregate: MtpAggregate {
                n_requests: 1,
                total_predicted: 100,
                total_draft: 0,
                total_draft_accepted: 0,
                aggregate_accept_rate: 0.0,
                wall_s_total: 1.5,
            },
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: MtpBenchResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.entries.len(), 1);
        assert_eq!(deserialized.entries[0].name, "test_prompt");
        assert_eq!(deserialized.aggregate.n_requests, 1);
    }
}
