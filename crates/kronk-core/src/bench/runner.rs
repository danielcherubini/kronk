//! Backend runner and benchmark orchestrator for LLM inference benchmarking.
//!
//! This module provides:
//! - `BenchBackend`: tracks a running backend process
//! - `start_backend`: spawn and health-check a backend
//! - `stop_backend`: gracefully stop a backend
//! - `run_benchmark`: orchestrate benchmark runs

use anyhow::{Context, Result};
use std::time::Instant;
use tokio::process::Command;

use crate::bench::{compute_summary, BenchConfig, BenchReport, BenchSummary, ModelInfo};
use crate::config::Config;
use crate::proxy::process::check_health;
use crate::proxy::process::{force_kill_process, is_process_alive, kill_process};
use tracing::info;

/// Information about a running backend
struct BenchBackend {
    pid: u32,
    url: String,
    load_time_ms: f64,
}

/// Detect GPU type from backend path and NVIDIA availability
fn detect_gpu_type(backend_path: &str, has_nvidia: bool) -> String {
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
fn extract_gpu_layers(args: &[String]) -> Option<String> {
    args.windows(2)
        .filter(|w| w[0] == "-ngl" || w[0] == "--n-gpu-layers")
        .map(|w| w[1].clone())
        .next()
        .map(|v| v.trim_matches('"').to_string())
}

/// Start a backend process and wait for it to be healthy
async fn start_backend(
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

    // Build full args
    let mut args = config.build_full_args(server_config, backend_config, ctx_override)?;
    override_arg(&mut args, "--host", "127.0.0.1");
    override_arg(&mut args, "--port", &port.to_string());

    let backend_path = &backend_config.path;
    let health_url = format!("http://127.0.0.1:{}/health", port);

    info!("Executing backend: {} {}", backend_path, args.join(" "));

    let mut child = Command::new(backend_path)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to spawn backend '{}'", backend_path))?;

    let pid = child
        .id()
        .ok_or_else(|| anyhow::anyhow!("Failed to get backend PID"))?;

    // Spawn reaper task
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    // Poll health every 500ms with 120 second timeout
    let timeout = std::time::Duration::from_secs(120);
    let start = Instant::now();

    while start.elapsed() < timeout {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        if let Ok(response) = check_health(&health_url, Some(30)).await {
            if response.status().is_success() {
                info!("Backend '{}' is healthy", server_name);
                break;
            }
        }

        tracing::debug!("Health check pending for backend: {}", server_name);
    }

    if start.elapsed() >= timeout {
        let _ = kill_process(pid).await;
        return Err(anyhow::anyhow!(
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
async fn stop_backend(backend: &BenchBackend) -> Result<()> {
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

/// Run a benchmark and return a complete report
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
        gpu_type: detect_gpu_type(&backend_config.path, crate::gpu::query_vram().is_some()),
        context_length: server_config.context_length,
        gpu_layers: extract_gpu_layers(&server_config.args),
    };

    // Start backend
    let backend = start_backend(config, server_name, bench_config.ctx_override).await?;

    println!("Backend loaded in {:.0} ms", backend.load_time_ms);

    // Run inner benchmark logic
    let summaries = run_benchmark_inner(&backend, bench_config).await;

    // Stop backend (don't propagate errors over measurement errors)
    stop_backend(&backend).await.ok();

    Ok(BenchReport {
        model_info,
        config: bench_config.clone(),
        summaries,
        load_time_ms: backend.load_time_ms,
        vram: crate::gpu::query_vram(),
    })
}

/// Inner benchmark logic that runs measurements
async fn run_benchmark_inner(
    backend: &BenchBackend,
    bench_config: &BenchConfig,
) -> Vec<BenchSummary> {
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

            let summary = compute_summary(&test_name, pp_size, tg_size, &measurements);
            summaries.push(summary);
        }
    }

    summaries
}

/// Override a CLI flag's value in an argument list
fn override_arg(args: &mut Vec<String>, flag: &str, value: &str) {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        if pos + 1 < args.len() {
            args[pos + 1] = value.to_string();
        } else {
            args.push(value.to_string());
        }
    } else {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_type_from_path_cuda() {
        let result = detect_gpu_type("llama-server-cuda", false);
        assert_eq!(result, "CUDA");
    }

    #[test]
    fn test_gpu_type_from_path_vulkan() {
        let result = detect_gpu_type("llama-server-vulkan", false);
        assert_eq!(result, "Vulkan");
    }

    #[test]
    fn test_gpu_type_from_path_default() {
        let result = detect_gpu_type("llama-server", false);
        assert_eq!(result, "CPU");
    }

    #[test]
    fn test_extract_gpu_layers_some() {
        let args = vec![
            "-m".to_string(),
            "model.gguf".to_string(),
            "-ngl".to_string(),
            "99".to_string(),
        ];
        let result = extract_gpu_layers(&args);
        assert_eq!(result, Some("99".to_string()));
    }

    #[test]
    fn test_extract_gpu_layers_none() {
        let args = vec!["-m".to_string(), "model.gguf".to_string()];
        let result = extract_gpu_layers(&args);
        assert_eq!(result, None);
    }
}
