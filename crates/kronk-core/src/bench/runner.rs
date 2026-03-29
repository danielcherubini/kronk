//! Backend runner and benchmark orchestrator for LLM inference benchmarking.
//!
//! This module provides:
//! - `BenchBackend`: tracks a running backend process
//! - `_start_backend`: spawn and health-check a backend
//! - `_stop_backend`: gracefully stop a backend
//! - `run_benchmark`: orchestrate benchmark runs

use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use tokio::process::Command;
use tracing::info;

use crate::bench::{compute_summary, BenchConfig, BenchReport, BenchSummary, ModelInfo};
use crate::config::Config;
use crate::proxy::process::{check_health, force_kill_process, is_process_alive, kill_process};

/// Information about a running backend
#[derive(Debug)]
struct BenchBackend {
    pid: u32,
    url: String,
    load_time_ms: f64,
}

/// Detect GPU type from backend path and NVIDIA availability
fn _detect_gpu_type(backend_path: &str, has_nvidia: bool) -> String {
    let path_lower = backend_path.to_lowercase();
    if path_lower.contains("vulkan") {
        "Vulkan".to_string()
    } else if path_lower.contains("cuda") {
        "CUDA".to_string()
    } else if path_lower.contains("rocm") {
        "ROCm".to_string()
    } else if has_nvidia {
        "CUDA".to_string()
    } else {
        "CPU".to_string()
    }
}

/// Extract GPU layers from args (next value after "-ngl")
fn _extract_gpu_layers(args: &[String]) -> Option<String> {
    args.windows(2)
        .filter(|w| w[0] == "-ngl" || w[0] == "--n-gpu-layers")
        .map(|w| w[1].clone())
        .next()
        .map(|v| v.trim_matches('"').to_string())
}

/// Override a CLI flag's value in an argument list.
///
/// Removes **all** existing occurrences of `flag` and its following value, then
/// appends a single canonical `flag value` pair.  This prevents duplicate flags
/// left by `build_full_args` from conflicting with the bench-specific values.
///
/// Two forms are recognised and stripped:
/// - Space-separated: `["--port", "8080"]` — removes both tokens.
/// - Inline-equals: `["--port=8080"]` — removes the single token.
fn _override_arg(args: &mut Vec<String>, flag: &str, value: &str) {
    let inline_prefix = format!("{}=", flag);
    let mut cleaned: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            // Space-separated form: skip the flag token and its value token.
            i += 1;
            if i < args.len() {
                i += 1;
            }
        } else if args[i].starts_with(&inline_prefix) {
            // Inline-equals form: skip the single combined token.
            i += 1;
        } else {
            cleaned.push(args[i].clone());
            i += 1;
        }
    }
    cleaned.push(flag.to_string());
    cleaned.push(value.to_string());
    *args = cleaned;
}

/// Start a backend process and wait for it to be healthy
async fn _start_backend(
    config: &Config,
    server_name: &str,
    ctx_override: Option<u32>,
) -> Result<BenchBackend> {
    info!("Starting backend for server: {}", server_name);

    let (server_config, backend_config) = config
        .resolve_server(server_name)
        .with_context(|| "Failed to resolve server config for bench")?;

    let spawn_start = Instant::now();

    // Allocate a free port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);

    // Build full args, then overwrite host/port removing any duplicates
    let mut args = config.build_full_args(server_config, backend_config, ctx_override)?;
    _override_arg(&mut args, "--host", "127.0.0.1");
    _override_arg(&mut args, "--port", &port.to_string());

    let backend_path = &backend_config.path;
    let health_url = format!("http://127.0.0.1:{}/health", port);

    info!("Executing backend: {} {}", backend_path, args.join(" "));

    let mut cmd = Command::new(backend_path);
    // Set working directory to the backend's parent dir so Windows can find
    // companion DLLs (ggml-cuda.dll, ggml.dll, etc.) alongside the binary.
    if let Some(parent) = std::path::Path::new(backend_path).parent() {
        if parent.is_dir() {
            cmd.current_dir(parent);
        }
    }
    let mut child = cmd
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to spawn backend '{}'", backend_path))?;

    let pid = child
        .id()
        .ok_or_else(|| anyhow!("Failed to get backend PID"))?;

    // Spawn reaper task
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    // Poll health every 500ms with 120 second timeout.
    // Use an explicit `healthy` flag so that a successful check on the very last
    // iteration is not mis-classified as a timeout by the post-loop guard.
    let timeout = std::time::Duration::from_secs(120);
    let start = Instant::now();
    let mut healthy = false;

    while start.elapsed() < timeout {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Fail fast if the process died before becoming healthy
        if !is_process_alive(pid) {
            return Err(anyhow!(
                "Backend '{}' (pid {}) exited before becoming healthy",
                server_name,
                pid
            ));
        }

        if let Ok(response) = check_health(&health_url, Some(30)).await {
            if response.status().is_success() {
                info!("Backend '{}' is healthy", server_name);
                healthy = true;
                break;
            }
        }

        tracing::debug!("Health check pending for backend: {}", server_name);
    }

    if !healthy {
        let _ = kill_process(pid).await;
        return Err(anyhow!(
            "Backend '{}' failed to become healthy after {}s",
            server_name,
            timeout.as_secs()
        ));
    }

    let load_time_ms = spawn_start.elapsed().as_secs_f64() * 1000.0;

    Ok(BenchBackend {
        pid,
        url: format!("http://127.0.0.1:{}", port),
        load_time_ms,
    })
}

/// Stop a backend process
async fn _stop_backend(backend: &BenchBackend) -> Result<()> {
    info!("Stopping backend (pid: {:?})", backend.pid);

    kill_process(backend.pid).await?;

    // Wait up to 5 seconds for process to exit
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if !is_process_alive(backend.pid) {
            info!("Backend process {:?} exited gracefully", backend.pid);
            break;
        }
        if Instant::now() >= deadline {
            tracing::warn!(
                "Backend process {:?} did not exit gracefully, forcing kill",
                backend.pid
            );
            force_kill_process(backend.pid).await?;
            break;
        }
    }

    Ok(())
}

/// Run a benchmark against a named server config and return a complete report.
///
/// Resolves the server configuration, spawns the backend process, runs warmup
/// and measurement iterations for every `(pp_size, tg_size)` combination in
/// `bench_config`, collects statistics, and tears the backend down before
/// returning.
///
/// # Parameters
/// - `config` — workspace [`Config`] used to resolve the server and backend settings.
/// - `server_name` — name of the server entry in `config.models` to benchmark.
/// - `bench_config` — benchmark parameters: PP/TG sizes, run counts, warmup
///   iterations, and optional context-size override.
///
/// # Returns
/// `Ok(BenchReport)` containing model metadata, per-combination summaries,
/// backend load time, and a VRAM snapshot taken just before shutdown.
///
/// # Errors
/// Returns `Err` if:
/// - `server_name` cannot be resolved in `config`.
/// - The backend process fails to start or does not become healthy within 120 s.
/// - All measurement runs for any `(pp_size, tg_size)` combination fail.
/// - The backend cannot be stopped cleanly after a successful benchmark run.
pub async fn run_benchmark(
    config: &Config,
    server_name: &str,
    bench_config: &BenchConfig,
) -> Result<BenchReport> {
    println!("Starting benchmark for '{}'...", server_name);

    // Build ModelInfo from config data
    let (server_config, backend_config) = config.resolve_server(server_name)?;
    let model_info = ModelInfo {
        name: server_name.to_string(),
        model_id: server_config.model.clone(),
        quant: server_config.quant.clone(),
        backend: server_config.backend.clone(),
        gpu_type: _detect_gpu_type(&backend_config.path, crate::gpu::query_vram().is_some()),
        context_length: bench_config.ctx_override.or(server_config.context_length),
        gpu_layers: _extract_gpu_layers(&server_config.args),
    };

    // Start backend
    let backend = _start_backend(config, server_name, bench_config.ctx_override).await?;

    println!("Backend loaded in {:.0} ms", backend.load_time_ms);

    // Run inner benchmark logic — always attempt to stop the backend regardless
    // of outcome, then surface errors from either phase.
    let inner_result = _run_benchmark_inner(&backend, bench_config).await;
    let stop_result = _stop_backend(&backend).await;

    // Prefer the measurement error; only surface the stop error when measurement
    // succeeded (avoids masking the root cause with a teardown error).
    let summaries = inner_result?;
    stop_result?;

    Ok(BenchReport {
        model_info,
        config: bench_config.clone(),
        summaries,
        load_time_ms: backend.load_time_ms,
        vram: crate::gpu::query_vram(),
    })
}

/// Inner benchmark logic that runs measurements
async fn _run_benchmark_inner(
    backend: &BenchBackend,
    bench_config: &BenchConfig,
) -> Result<Vec<BenchSummary>> {
    let mut summaries = Vec::new();

    // For each pp_size × tg_size combination
    for &pp_size in &bench_config.pp_sizes {
        for &tg_size in &bench_config.tg_sizes {
            let test_name = format!("pp{}/tg{}", pp_size, tg_size);
            println!(
                "Running {} (warmup: {}, runs: {})...",
                test_name, bench_config.warmup, bench_config.runs
            );

            // Warmup phase - discard results
            for _ in 0..bench_config.warmup {
                let _ =
                    crate::bench::measure::send_bench_request(&backend.url, pp_size, tg_size).await;
            }

            // Measurement phase
            let mut measurements = Vec::with_capacity(bench_config.runs as usize);
            for run_idx in 0..bench_config.runs {
                match crate::bench::measure::send_bench_request(&backend.url, pp_size, tg_size)
                    .await
                {
                    Ok(measurement) => measurements.push(measurement),
                    Err(e) => tracing::warn!("Measurement run {} failed: {}", run_idx + 1, e),
                }
            }

            if measurements.is_empty() {
                return Err(anyhow!(
                    "All {} measurement run(s) failed for {} — no results to summarize",
                    bench_config.runs,
                    test_name
                ));
            }

            let summary = compute_summary(&test_name, pp_size, tg_size, &measurements);
            summaries.push(summary);
        }
    }

    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that a backend binary path containing "cuda" is detected as CUDA.
    #[test]
    fn test_gpu_type_from_path_cuda() {
        let result = _detect_gpu_type("llama-server-cuda", false);
        assert_eq!(result, "CUDA");
    }

    /// Verifies that a backend binary path containing "vulkan" is detected as Vulkan.
    #[test]
    fn test_gpu_type_from_path_vulkan() {
        let result = _detect_gpu_type("llama-server-vulkan", false);
        assert_eq!(result, "Vulkan");
    }

    /// Verifies that an unrecognised backend path without NVIDIA presence defaults to CPU.
    #[test]
    fn test_gpu_type_from_path_default() {
        let result = _detect_gpu_type("llama-server", false);
        assert_eq!(result, "CPU");
    }

    /// Verifies that `_extract_gpu_layers` returns the value following "-ngl" in the args list.
    #[test]
    fn test_extract_gpu_layers_some() {
        let args = vec![
            "-m".to_string(),
            "model.gguf".to_string(),
            "-ngl".to_string(),
            "99".to_string(),
        ];
        let result = _extract_gpu_layers(&args);
        assert_eq!(result, Some("99".to_string()));
    }

    /// Verifies that `_extract_gpu_layers` returns `None` when "-ngl" is absent from the args list.
    #[test]
    fn test_extract_gpu_layers_none() {
        let args = vec!["-m".to_string(), "model.gguf".to_string()];
        let result = _extract_gpu_layers(&args);
        assert_eq!(result, None);
    }

    /// Verifies that `_override_arg` replaces an existing flag value in place.
    #[test]
    fn test_override_arg_replaces_existing() {
        let mut args = vec![
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            "8080".to_string(),
        ];
        _override_arg(&mut args, "--host", "127.0.0.1");
        assert_eq!(
            args,
            vec![
                "--port".to_string(),
                "8080".to_string(),
                "--host".to_string(),
                "127.0.0.1".to_string(),
            ]
        );
    }

    /// Verifies that `_override_arg` removes duplicate flag occurrences, leaving exactly one.
    #[test]
    fn test_override_arg_removes_duplicates() {
        let mut args = vec![
            "--port".to_string(),
            "8080".to_string(),
            "--port".to_string(),
            "9090".to_string(),
        ];
        _override_arg(&mut args, "--port", "54321");
        let port_count = args.iter().filter(|a| a.as_str() == "--port").count();
        assert_eq!(port_count, 1);
        let pos = args.iter().position(|a| a == "--port").unwrap();
        assert_eq!(args[pos + 1], "54321");
    }

    /// Verifies that `_override_arg` appends a new flag when not already present.
    #[test]
    fn test_override_arg_appends_when_absent() {
        let mut args = vec!["--model".to_string(), "foo.gguf".to_string()];
        _override_arg(&mut args, "--host", "127.0.0.1");
        assert!(args.contains(&"--host".to_string()));
        let pos = args.iter().position(|a| a == "--host").unwrap();
        assert_eq!(args[pos + 1], "127.0.0.1");
    }

    /// Verifies that `_override_arg` removes inline `--flag=value` tokens.
    #[test]
    fn test_override_arg_removes_inline_equals_form() {
        let mut args = vec![
            "--model".to_string(),
            "foo.gguf".to_string(),
            "--port=8080".to_string(),
        ];
        _override_arg(&mut args, "--port", "54321");
        // The inline token must be gone
        assert!(!args.iter().any(|a| a.starts_with("--port=")));
        // Exactly one --port flag must be present with the correct value
        let port_count = args.iter().filter(|a| a.as_str() == "--port").count();
        assert_eq!(port_count, 1);
        let pos = args.iter().position(|a| a == "--port").unwrap();
        assert_eq!(args[pos + 1], "54321");
    }
}
