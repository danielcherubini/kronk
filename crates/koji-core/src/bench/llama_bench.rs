//! llama-bench integration for benchmarking GGUF files directly.
//!
//! Wraps the llama-bench binary from llama.cpp's tools/ directory.
//! Runs raw inference benchmarks without spawning a server.

use crate::backends::ProgressSink;
use crate::bench::{BenchConfig, BenchReport, BenchSummary, ModelInfo};
use crate::config::Config;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

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
/// 3. llama-bench on PATH (system install)
pub fn find_llama_bench(backend_path: &std::path::Path) -> Result<PathBuf> {
    // 1. Check env var first
    if let Ok(p) = std::env::var("LLAMA_BENCH_PATH") {
        let p = std::path::PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }

    // 2. Check same directory as backend binary
    // llama-bench may be placed alongside llama-server (e.g. ~/.config/koji/backends/.../llama-bench)
    let bench_name = if cfg!(target_os = "windows") {
        "llama-bench.exe"
    } else {
        "llama-bench"
    };
    if let Some(parent_dir) = backend_path.parent() {
        let direct_path = parent_dir.join(bench_name);
        if direct_path.exists() {
            return Ok(direct_path);
        }
    }

    // 3. Check grandparent's tools directory
    // Backend path is typically: /path/to/llama-server (binary)
    // Parent is: /path/to/ (bin dir)
    // Grandparent is: /path/to/llama.cpp/ or similar
    // Tools are at: <grandparent>/tools/llama-bench
    let grandparent = backend_path.parent().and_then(|p| p.parent());

    if let Some(parent_dir) = grandparent {
        let tools_dir = parent_dir.join("tools");
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
fn build_args(model_path: &std::path::Path, config: &LlamaBenchConfig) -> Vec<String> {
    let mut args = Vec::new();

    // Model file(s) — llama-bench accepts --model for single model
    args.push("--model".to_string());
    args.push(model_path.to_string_lossy().to_string());

    // Prompt sizes: single value with -p, multiple comma-separated
    if config.pp_sizes.len() == 1 {
        args.push("-p".to_string());
        args.push(config.pp_sizes[0].to_string());
    } else {
        args.push("-p".to_string());
        args.push(
            config
                .pp_sizes
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
    }

    // Generation lengths: single value with -n, multiple comma-separated
    if config.tg_sizes.len() == 1 {
        args.push("-n".to_string());
        args.push(config.tg_sizes[0].to_string());
    } else {
        args.push("-n".to_string());
        args.push(
            config
                .tg_sizes
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
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
            args.push(
                threads
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            );
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
fn parse_bench_json(output: &str) -> Result<Vec<BenchSummary>> {
    let entries: serde_json::Value =
        serde_json::from_str(output).context("Failed to parse llama-bench JSON output")?;

    let arr = entries
        .as_array()
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
            ttft_mean: 0.0, // llama-bench doesn't measure TTFT
            ttft_stddev: 0.0,
            total_mean: 0.0, // llama-bench doesn't measure total latency
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
/// `backend_name` is an optional override — if provided, llama-bench is
/// resolved from that backend's installation path instead of the model's
/// configured backend.
///
/// This function is designed to be called from a background job — it streams
/// progress via the provided ProgressSink.
pub async fn run_llama_bench(
    config: &Config,
    model_id: &str,
    backend_name: Option<&str>,
    bench_config: &LlamaBenchConfig,
    progress: &dyn ProgressSink,
) -> Result<BenchReport> {
    use crate::db::OpenResult;

    // Resolve the model config to get model path and backend info
    let db_dir = Config::config_dir()?;
    let OpenResult { conn, .. } = crate::db::open(&db_dir)?;
    let model_configs = crate::db::load_model_configs(&conn)?;

    // If model_id is an integer db_id, resolve it to the config key first.
    let resolved_id = if let Ok(db_id) = model_id.parse::<i64>() {
        model_configs
            .iter()
            .find(|(_, mc)| mc.db_id == Some(db_id))
            .map(|(key, _)| key.as_str())
            .unwrap_or(model_id)
    } else {
        model_id
    };

    let (server_config, _backend_config) = config
        .resolve_server(&model_configs, resolved_id)
        .context("Failed to resolve server config for benchmark")?;

    // Get the model file path from the model config's first model file
    let model_path = {
        // Look up the model by resolved config key
        let mc = model_configs
            .get(resolved_id)
            .with_context(|| format!("Model config '{}' not found", resolved_id))?;
        // Use db_id to look up the DB record for file lookup
        let rec_id = mc.db_id.context("Model config has no db_id")?;
        let record = crate::db::queries::get_model_config(&conn, rec_id)?.with_context(|| {
            format!("Model config record (id={}) not found in database", rec_id)
        })?;
        // Find the first model file (.gguf extension) for this config
        let files = crate::db::queries::get_model_files(&conn, record.id)?;
        let model_file = files
            .into_iter()
            .find(|f| f.filename.ends_with(".gguf"))
            .context("No .gguf model file found for this config")?;

        // Build full path: models_dir + repo_id + filename
        let model_data_dir = config.models_dir()?;
        let candidate = model_data_dir
            .join(&record.repo_id)
            .join(&model_file.filename);
        if candidate.exists() {
            candidate
        } else {
            // Fallback: try db_dir/models (legacy location)
            let legacy = db_dir.join("models");
            let legacy_candidate = legacy.join(&record.repo_id).join(&model_file.filename);
            if legacy_candidate.exists() {
                legacy_candidate
            } else {
                bail!(
                    "Model file not found: {} (searched {:?} and {:?})",
                    model_file.filename,
                    candidate,
                    legacy_candidate
                )
            }
        }
    };

    // Resolve backend binary path — use explicit backend_name if provided,
    // otherwise fall back to the model's configured backend.
    let target_backend = backend_name.unwrap_or(&server_config.backend);
    let backend_path = {
        let conn = Config::open_db();
        config.resolve_backend_path(target_backend, &conn)?
    };

    // Find llama-bench binary
    let bench_binary =
        find_llama_bench(&backend_path).context(format!(
            "llama-bench not found for backend '{}'. Install llama.cpp from source or set LLAMA_BENCH_PATH",
            target_backend
        ))?;

    // Get llama-bench version for reporting
    let _version_output = Command::new(&bench_binary)
        .arg("--version")
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    progress.log(&format!("Using llama-bench: {}", bench_binary.display()));
    progress.log(&format!("Model: {} ({})", model_id, model_path.display()));

    // Build command arguments
    let args = build_args(&model_path, bench_config);

    progress.log(&format!(
        "Running: {} {}",
        bench_binary.display(),
        args.join(" ")
    ));

    // Run llama-bench
    let start_time = std::time::Instant::now();

    let output = Command::new(&bench_binary)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute llama-bench")?;

    let _duration = start_time.elapsed();

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
        bail!(
            "llama-bench exited with error (code {}): {}",
            output.status,
            stderr
        );
    }

    // Parse JSON output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let summaries = parse_bench_json(&stdout)?;

    // Stream results to client via progress sink
    if let Ok(results_json) = serde_json::to_string(&summaries) {
        progress.result(&results_json);
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `find_llama_bench` returns an error when no binary is found.
    #[test]
    fn test_find_llama_bench_not_found() {
        // Use a path that definitely doesn't exist to test the fallback behavior
        let nonexistent = std::path::PathBuf::from("/nonexistent/path/llama-server");
        let result = find_llama_bench(&nonexistent);
        assert!(result.is_err());
    }

    /// Verifies that `build_args` produces correct arguments for a single PP and TG size.
    #[test]
    fn test_build_args_single_sizes() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![512],
            tg_sizes: vec![128],
            runs: 3,
            warmup: 1,
            threads: None,
            ngl_range: None,
            ctx_override: None,
        };

        let args = build_args(&model_path, &config);

        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"/test/model.gguf".to_string()));
        // Single values use short flags
        assert!(args.iter().position(|a| a == "-p").is_some());
        assert!(args.iter().position(|a| a == "512").is_some());
        assert!(args.iter().position(|a| a == "-n").is_some());
        assert!(args.iter().position(|a| a == "128").is_some());
        // Repetitions
        assert!(args.contains(&"-r".to_string()));
        assert!(args.contains(&"3".to_string()));
        // JSON output
        assert!(args.contains(&"-o".to_string()));
        assert!(args.contains(&"json".to_string()));
    }

    /// Verifies that `build_args` produces comma-separated values for multiple sizes.
    #[test]
    fn test_build_args_multiple_sizes() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![256, 512],
            tg_sizes: vec![64, 128, 256],
            runs: 5,
            warmup: 2,
            threads: Some(vec![4, 8]),
            ngl_range: Some("0-99+1".to_string()),
            ctx_override: Some(4096),
        };

        let args = build_args(&model_path, &config);

        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"256,512".to_string()));
        assert!(args.contains(&"-n".to_string()));
        assert!(args.contains(&"64,128,256".to_string()));
        assert!(args.contains(&"--threads".to_string()));
        assert!(args.contains(&"4,8".to_string()));
        assert!(args.contains(&"--n-gpu-layers".to_string()));
        assert!(args.contains(&"0-99+1".to_string()));
        assert!(args.contains(&"--fit-ctx".to_string()));
        assert!(args.contains(&"4096".to_string()));
    }

    /// Verifies that `parse_bench_json` correctly parses a single PP test entry.
    #[test]
    fn test_parse_bench_json_pp_test() {
        let json = r#"[{
            "n_prompt": 512,
            "n_gen": 0,
            "avg_ts": 5120.5,
            "stddev_ts": 42.3
        }]"#;

        let summaries = parse_bench_json(json).unwrap();

        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.test_name, "pp512");
        assert_eq!(s.prompt_tokens, 512);
        assert_eq!(s.gen_tokens, 0);
        assert!((s.pp_mean - 5120.5).abs() < 0.01);
        assert!((s.pp_stddev - 42.3).abs() < 0.01);
        assert_eq!(s.tg_mean, 0.0);
        assert_eq!(s.ttft_mean, 0.0);
    }

    /// Verifies that `parse_bench_json` correctly parses a single TG test entry.
    #[test]
    fn test_parse_bench_json_tg_test() {
        let json = r#"[{
            "n_prompt": 0,
            "n_gen": 128,
            "avg_ts": 1000.0,
            "stddev_ts": 15.5
        }]"#;

        let summaries = parse_bench_json(json).unwrap();

        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.test_name, "tg128");
        assert_eq!(s.prompt_tokens, 0);
        assert_eq!(s.gen_tokens, 128);
        assert_eq!(s.pp_mean, 0.0);
        assert!((s.tg_mean - 1000.0).abs() < 0.01);
    }

    /// Verifies that `parse_bench_json` correctly parses a combined PP+TG test entry.
    #[test]
    fn test_parse_bench_json_combined_test() {
        let json = r#"[{
            "n_prompt": 512,
            "n_gen": 128,
            "avg_ts": 950.0,
            "stddev_ts": 10.0
        }]"#;

        let summaries = parse_bench_json(json).unwrap();

        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.test_name, "pg512-128");
        assert_eq!(s.prompt_tokens, 512);
        assert_eq!(s.gen_tokens, 128);
    }

    /// Verifies that `parse_bench_json` returns an error for invalid JSON.
    #[test]
    fn test_parse_bench_json_invalid() {
        let result = parse_bench_json("not json");
        assert!(result.is_err());
    }

    /// Verifies that `parse_bench_json` returns an error for non-array JSON.
    #[test]
    fn test_parse_bench_json_not_array() {
        let result = parse_bench_json(r#"{"key": "value"}"#);
        assert!(result.is_err());
    }

    /// Verifies that `parse_bench_json` handles multiple entries correctly.
    #[test]
    fn test_parse_bench_json_multiple_entries() {
        let json = r#"[
            {"n_prompt": 512, "n_gen": 0, "avg_ts": 5000.0, "stddev_ts": 30.0},
            {"n_prompt": 0, "n_gen": 128, "avg_ts": 1000.0, "stddev_ts": 15.0},
            {"n_prompt": 512, "n_gen": 128, "avg_ts": 900.0, "stddev_ts": 20.0}
        ]"#;

        let summaries = parse_bench_json(json).unwrap();

        assert_eq!(summaries.len(), 3);
        assert_eq!(summaries[0].test_name, "pp512");
        assert_eq!(summaries[1].test_name, "tg128");
        assert_eq!(summaries[2].test_name, "pg512-128");
    }

    /// Verifies that `parse_bench_json` handles an empty JSON array.
    #[test]
    fn test_parse_bench_json_empty_array() {
        let json = "[]";
        let summaries = parse_bench_json(json).unwrap();
        assert!(summaries.is_empty());
    }

    /// Verifies that `build_args` produces comma-separated values for multiple sizes.
    #[test]
    fn test_build_args_multi_values() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![128, 256, 512],
            tg_sizes: vec![32, 64],
            runs: 5,
            warmup: 2,
            threads: None,
            ngl_range: None,
            ctx_override: None,
        };

        let args = build_args(&model_path, &config);

        assert!(args.contains(&"-p".to_string()));
        // Multi values should be comma-separated
        let pp_idx = args.iter().position(|a| a == "-p").unwrap();
        assert_eq!(args[pp_idx + 1], "128,256,512");
        let tg_idx = args.iter().position(|a| a == "-n").unwrap();
        assert_eq!(args[tg_idx + 1], "32,64");
    }

    /// Verifies that `build_args` handles a single thread count correctly.
    #[test]
    fn test_build_args_with_single_thread() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![512],
            tg_sizes: vec![128],
            runs: 3,
            warmup: 1,
            threads: Some(vec![4]),
            ngl_range: None,
            ctx_override: None,
        };

        let args = build_args(&model_path, &config);

        assert!(args.contains(&"--threads".to_string()));
        let threads_idx = args.iter().position(|a| a == "--threads").unwrap();
        assert_eq!(args[threads_idx + 1], "4");
    }

    /// Verifies that `detect_gpu_type` identifies CUDA from path.
    #[test]
    fn test_detect_gpu_type_cuda() {
        let path = std::path::PathBuf::from("/path/to/llama-server-cuda");
        assert_eq!(detect_gpu_type(&path), "CUDA");
    }

    /// Verifies that `detect_gpu_type` identifies Vulkan from path.
    #[test]
    fn test_detect_gpu_type_vulkan() {
        let path = std::path::PathBuf::from("/path/to/llama-server-vulkan");
        assert_eq!(detect_gpu_type(&path), "Vulkan");
    }

    /// Verifies that `detect_gpu_type` identifies ROCm from path.
    #[test]
    fn test_detect_gpu_type_rocm() {
        let path = std::path::PathBuf::from("/path/to/llama-server-rocm");
        assert_eq!(detect_gpu_type(&path), "ROCm");
    }

    /// Verifies that `detect_gpu_type` identifies Metal from path.
    #[test]
    fn test_detect_gpu_type_metal() {
        let path = std::path::PathBuf::from("/path/to/llama-server-metal");
        assert_eq!(detect_gpu_type(&path), "Metal");
    }

    /// Verifies that `detect_gpu_type` returns a valid GPU type string for unknown paths.
    /// On systems with CUDA, it returns "CUDA"; otherwise "CPU".
    #[test]
    fn test_detect_gpu_type_unknown_path() {
        let path = std::path::PathBuf::from("/path/to/llama-server");
        let result = detect_gpu_type(&path);
        assert!(matches!(result.as_str(), "CUDA" | "CPU"));
    }
}
