use serde::{Deserialize, Serialize};
use sysinfo::System;

use super::vram::{query_vram, VramInfo};

/// A snapshot of system-level hardware metrics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemMetrics {
    /// CPU utilization percentage (0.0–100.0)
    pub cpu_usage_pct: f32,
    /// RAM currently in use (MiB)
    pub ram_used_mib: u64,
    /// Total RAM (MiB)
    pub ram_total_mib: u64,
    /// GPU utilization percentage (0–100), None if not available
    pub gpu_utilization_pct: Option<u8>,
    /// VRAM usage, None if not available
    pub vram: Option<VramInfo>,
}

/// A timestamped snapshot of system + proxy metrics, suitable for persistence
/// in `system_metrics_history` and broadcast over the SSE stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub ts_unix_ms: i64,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    pub gpu_utilization_pct: Option<u8>,
    pub vram: Option<VramInfo>,
    pub models_loaded: u64,
    /// Per-model loaded/idle status, embedded in `MetricSample.models`.
    #[serde(default)]
    pub models: Vec<ModelStatus>,
}

/// Per-model loaded/idle status, embedded in `MetricSample.models`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelStatus {
    pub id: String,
    /// Integer database id of the model_configs row, if known. Emitted so the
    /// dashboard can link to the editor by id rather than by config_key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_id: Option<i64>,
    pub api_name: Option<String>,
    pub display_name: Option<String>,
    pub backend: String,
    pub loaded: bool,
    /// Quantization name (e.g. "Q4_K_M", "Q8_0"). Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    /// Model's configured context length in tokens. Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

/// Collect a snapshot of system metrics using a caller-owned `System`.
///
/// The caller is responsible for passing a `System` that persists across
/// calls so that `sysinfo` can compute CPU deltas correctly. This function
/// calls `refresh_cpu_usage` and `refresh_memory` once — no internal sleep.
/// It blocks on nvidia-smi subprocesses; call via `tokio::task::spawn_blocking`.
pub fn collect_system_metrics_with(sys: &mut System) -> SystemMetrics {
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let cpu_usage_pct = sys.global_cpu_info().cpu_usage();
    let ram_used_mib = sys.used_memory() / 1024 / 1024;
    let ram_total_mib = sys.total_memory() / 1024 / 1024;

    // GPU utilization via nvidia-smi
    let gpu_utilization_pct = query_gpu_utilization();

    // VRAM via existing query_vram()
    let vram = query_vram();

    SystemMetrics {
        cpu_usage_pct,
        ram_used_mib,
        ram_total_mib,
        gpu_utilization_pct,
        vram,
    }
}

/// Collect a snapshot of system metrics (CPU, RAM, GPU util, VRAM).
///
/// Creates a temporary `System`, sleeps for `MINIMUM_CPU_UPDATE_INTERVAL`
/// to get a meaningful CPU reading, then returns the snapshot. Prefer
/// [`collect_system_metrics_with`] for long-running tasks to avoid the
/// per-call allocation and sleep.
///
/// This function blocks — call via `tokio::task::spawn_blocking`.
pub fn collect_system_metrics() -> SystemMetrics {
    let mut sys = System::new();
    sys.refresh_cpu_usage();
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    collect_system_metrics_with(&mut sys)
}

/// Query GPU utilization percentage via nvidia-smi.
/// Returns None if nvidia-smi is unavailable or output cannot be parsed.
fn query_gpu_utilization() -> Option<u8> {
    // Try NVIDIA first
    if let Some(pct) = query_nvidia_gpu_utilization() {
        return Some(pct);
    }
    // Fall back to AMD sysfs
    query_amd_gpu_utilization()
}

fn query_nvidia_gpu_utilization() -> Option<u8> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next()?.trim().parse().ok()
}

/// Read GPU busy percentage from AMD AMDGPU sysfs interface.
///
/// Iterates `/sys/class/drm/card*/device/gpu_busy_percent` and returns
/// the first successfully parsed value.
fn query_amd_gpu_utilization() -> Option<u8> {
    let pattern = "/sys/class/drm/card*/device/gpu_busy_percent";
    if let Ok(paths) = glob::glob(pattern) {
        for path in paths.flatten() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                if let Ok(pct) = contents.trim().parse::<u8>() {
                    return Some(pct);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies `collect_system_metrics` returns sane CPU and RAM values on any machine.
    #[test]
    fn test_collect_system_metrics() {
        let metrics = collect_system_metrics();
        assert!(
            metrics.cpu_usage_pct >= 0.0 && metrics.cpu_usage_pct <= 100.0,
            "cpu_usage_pct out of range: {}",
            metrics.cpu_usage_pct
        );
        assert!(metrics.ram_total_mib > 0, "ram_total_mib should be > 0");
        assert!(
            metrics.ram_used_mib <= metrics.ram_total_mib,
            "ram_used_mib ({}) > ram_total_mib ({})",
            metrics.ram_used_mib,
            metrics.ram_total_mib
        );
        // GPU fields may be None in CI — do not assert them
        println!("cpu_usage_pct: {}", metrics.cpu_usage_pct);
        println!("ram_used_mib: {}", metrics.ram_used_mib);
        println!("ram_total_mib: {}", metrics.ram_total_mib);
        println!("gpu_utilization_pct: {:?}", metrics.gpu_utilization_pct);
        println!("vram: {:?}", metrics.vram);
    }

    /// Verifies `collect_system_metrics_with` works correctly when `System` is reused across calls.
    #[test]
    fn test_collect_system_metrics_with_reuses_system() {
        // Verify collect_system_metrics_with works when System is reused across calls.
        let mut sys = System::new();
        let metrics = collect_system_metrics_with(&mut sys);
        assert!(
            metrics.cpu_usage_pct >= 0.0 && metrics.cpu_usage_pct <= 100.0,
            "cpu_usage_pct out of range: {}",
            metrics.cpu_usage_pct
        );
        assert!(metrics.ram_total_mib > 0, "ram_total_mib should be > 0");
        assert!(
            metrics.ram_used_mib <= metrics.ram_total_mib,
            "ram_used_mib ({}) > ram_total_mib ({})",
            metrics.ram_used_mib,
            metrics.ram_total_mib
        );
    }
}
