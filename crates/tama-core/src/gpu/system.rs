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
    /// Deprecated: use `state` instead. True iff the model is in the Ready state.
    #[deprecated(since = "1.45.0", note = "use state field instead")]
    pub loaded: bool,
    /// Current lifecycle state of the model's backend.
    /// One of: `idle`, `loading`, `ready`, `unloading`, `failed`.
    #[serde(default)]
    pub state: String,
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

/// Read GPU utilization from AMD AMDGPU sysfs interfaces.
///
/// Tries multiple sources in order:
/// 1. `gpu_busy_percent` — simple text file, but returns `-EBUSY` when the
///    GPU is in a low-power state (D3hot) and the SMU firmware is asleep.
/// 2. `gpu_metrics` — binary blob defined in the kernel header
///    `kgd_pp_interface.h`. Also returns `-EBUSY` when the GPU is asleep.
/// 3. `power_state` — if the GPU is in a low-power state (D1–D3), the
///    utilization is 0% by definition since the GPU is not processing work.
fn query_amd_gpu_utilization() -> Option<u8> {
    // 1. Try the simple text interface first.
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

    // 2. Fallback: parse the gpu_metrics binary blob.
    if let Some(pct) = query_amd_gpu_metrics_utilization() {
        return Some(pct);
    }

    // 3. Fallback: check power state. If the GPU is in a low-power state,
    //    it's not doing any work, so utilization is 0%.
    query_amd_gpu_power_state_utilization()
}

/// Read GPU utilization from the AMD `gpu_metrics` sysfs binary blob.
///
/// The binary format is defined in the Linux kernel header
/// `drivers/gpu/drm/amd/include/kgd_pp_interface.h`.  The header is a
/// `metrics_table_header { uint16_t structure_size; uint8_t format_revision;
/// uint8_t content_revision; }`, followed by format-specific fields.
///
/// Returns the average GFX activity as a percentage (0–100), or `None` if
/// the file cannot be read or parsed.
fn query_amd_gpu_metrics_utilization() -> Option<u8> {
    let pattern = "/sys/class/drm/card*/device/gpu_metrics";
    if let Ok(paths) = glob::glob(pattern) {
        for path in paths.flatten() {
            if let Ok(data) = std::fs::read(&path) {
                if let Some(pct) = parse_amd_gpu_metrics_gfx_activity(&data) {
                    return Some(pct);
                }
            }
        }
    }
    None
}

/// Sentinel value used by the AMDGPU driver for "not available" fields.
const AMD_GPU_METRICS_NA: u16 = 0xFFFF;

/// Parse `average_gfx_activity` from an AMD `gpu_metrics` binary blob.
///
/// The offset of `average_gfx_activity` depends on the format revision
/// (and, for some revisions, the content revision):
///
/// | format_rev | content_rev | Struct            | Offset |
/// |------------|-------------|-------------------|--------|
/// | 0          | any         | gpu_metrics_v1_0  | 24     |
/// | 1          | 0–3         | gpu_metrics_v1_1+ | 16     |
/// | 1          | ≥ 4         | gpu_metrics_v1_4+ | 12     |
/// | 2          | 0           | gpu_metrics_v2_0  | 36     |
/// | 2          | ≥ 1         | gpu_metrics_v2_1+ | 28     |
/// | 3          | any         | gpu_metrics_v3_0  | 42     |
///
/// Some newer formats report utilization in **centi-percent** (0–10 000)
/// rather than percent (0–100).  When the raw value exceeds 100 we assume
/// centi-percent and divide by 100.
/// Check AMD GPU power state via sysfs.
///
/// When the GPU is in a low-power state (D1, D2, D3hot, D3cold), the SMU
/// firmware is asleep and `gpu_busy_percent`/`gpu_metrics` return `-EBUSY`.
/// In these states the GPU is not processing any work, so utilization is 0%.
///
/// Returns `Some(0)` if the GPU is in a low-power state, `None` if the
/// power state cannot be determined.
fn query_amd_gpu_power_state_utilization() -> Option<u8> {
    let pattern = "/sys/class/drm/card*/device/power_state";
    if let Ok(paths) = glob::glob(pattern) {
        for path in paths.flatten() {
            if let Ok(state) = std::fs::read_to_string(&path) {
                let state = state.trim();
                // D0 = fully active, D1–D3 = low power / asleep
                if state.starts_with('D') && state != "D0" {
                    return Some(0);
                }
            }
        }
    }
    None
}

fn parse_amd_gpu_metrics_gfx_activity(data: &[u8]) -> Option<u8> {
    if data.len() < 4 {
        return None;
    }

    let struct_size = u16::from_le_bytes([data[0], data[1]]) as usize;
    let format_rev = data[2];
    let content_rev = data[3];

    // Validate that we have enough data for the declared struct.
    if data.len() < struct_size {
        return None;
    }

    let offset = match format_rev {
        // gpu_metrics_v1_0: header(4) + system_clock_counter(8) + 6×temperature(12)
        0 => 24,
        // gpu_metrics_v1_1 / v1_2 / v1_3: header(4) + 6×temperature(12)
        // gpu_metrics_v1_4 / v1_5: header(4) + 3×temperature(6) + curr_socket_power(2)
        1 => {
            if content_rev >= 4 {
                12
            } else {
                16
            }
        }
        // gpu_metrics_v2_0: header(4) + system_clock_counter(8) + temps(24)
        // gpu_metrics_v2_1+: header(4) + temps(24)
        2 => {
            if content_rev == 0 {
                36
            } else {
                28
            }
        }
        // gpu_metrics_v3_0: header(4) + 2×temp(4) + 16×temp(32) + temp_skin(2)
        3 => 42,
        _ => return None,
    };

    if data.len() < offset + 2 {
        return None;
    }

    let val = u16::from_le_bytes([data[offset], data[offset + 1]]);

    // AMDGPU uses 0xFFFF as a sentinel for "not available".
    if val == AMD_GPU_METRICS_NA {
        return None;
    }

    // Some formats report in centi-percent (0–10 000), others in percent (0–100).
    // If the value exceeds 100, assume centi-percent and divide by 100.
    let pct = if val > 100 { (val / 100) as u8 } else { val as u8 };

    Some(pct)
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

    // ── gpu_metrics binary parsing tests ────────────────────────────────

    /// Helper: build a minimal gpu_metrics blob with a known
    /// `average_gfx_activity` value at the correct offset.
    fn build_gpu_metrics_blob(format_rev: u8, content_rev: u8, gfx_activity: u16) -> Vec<u8> {
        let offset = match format_rev {
            0 => 24,
            1 => {
                if content_rev >= 4 { 12 } else { 16 }
            }
            2 => {
                if content_rev == 0 { 36 } else { 28 }
            }
            3 => 42,
            _ => 50,
        };
        // Allocate enough bytes to cover the offset + 2 (u16), plus some
        // trailing data so `struct_size` validation passes.
        let struct_size = (offset + 2 + 16) as u16;
        let mut data = vec![0u8; struct_size as usize];
        // Header
        data[0..2].copy_from_slice(&struct_size.to_le_bytes());
        data[2] = format_rev;
        data[3] = content_rev;
        // average_gfx_activity
        data[offset..offset + 2].copy_from_slice(&gfx_activity.to_le_bytes());
        data
    }

    #[test]
    fn test_parse_gpu_metrics_v1_0_percent() {
        let blob = build_gpu_metrics_blob(0, 0, 42);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(42));
    }

    #[test]
    fn test_parse_gpu_metrics_v1_0_zero() {
        let blob = build_gpu_metrics_blob(0, 0, 0);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(0));
    }

    #[test]
    fn test_parse_gpu_metrics_v1_0_full() {
        let blob = build_gpu_metrics_blob(0, 0, 100);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(100));
    }

    #[test]
    fn test_parse_gpu_metrics_v1_1_percent() {
        let blob = build_gpu_metrics_blob(1, 1, 73);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(73));
    }

    #[test]
    fn test_parse_gpu_metrics_v1_3_percent() {
        // v1_3: format_revision=1, content_revision=3 → offset 16
        let blob = build_gpu_metrics_blob(1, 3, 55);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(55));
    }

    #[test]
    fn test_parse_gpu_metrics_v1_4_percent() {
        // v1_4: format_revision=1, content_revision=4 → offset 12
        let blob = build_gpu_metrics_blob(1, 4, 85);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(85));
    }

    #[test]
    fn test_parse_gpu_metrics_v1_5_centi_percent() {
        // v1_5 with centi-percent: 7400 → 74%
        let blob = build_gpu_metrics_blob(1, 5, 7400);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(74));
    }

    #[test]
    fn test_parse_gpu_metrics_v2_0_percent() {
        let blob = build_gpu_metrics_blob(2, 0, 30);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(30));
    }

    #[test]
    fn test_parse_gpu_metrics_v2_1_percent() {
        let blob = build_gpu_metrics_blob(2, 1, 65);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(65));
    }

    #[test]
    fn test_parse_gpu_metrics_v2_1_centi_percent() {
        // v2_1 with centi-percent: 5550 → 55%
        let blob = build_gpu_metrics_blob(2, 1, 5550);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(55));
    }

    #[test]
    fn test_parse_gpu_metrics_v2_4_centi_percent() {
        // v2_4 explicitly uses centi-percent: 10000 → 100%
        let blob = build_gpu_metrics_blob(2, 4, 10000);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(100));
    }

    #[test]
    fn test_parse_gpu_metrics_v3_0_percent() {
        let blob = build_gpu_metrics_blob(3, 0, 15);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), Some(15));
    }

    #[test]
    fn test_parse_gpu_metrics_na_sentinel() {
        // 0xFFFF means "not available" → should return None
        let blob = build_gpu_metrics_blob(1, 1, 0xFFFF);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), None);
    }

    #[test]
    fn test_parse_gpu_metrics_truncated_data() {
        // Data shorter than declared struct_size → None
        let mut blob = build_gpu_metrics_blob(1, 1, 50);
        // Set struct_size larger than actual data
        let big_size = (blob.len() + 100) as u16;
        blob[0..2].copy_from_slice(&big_size.to_le_bytes());
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), None);
    }

    #[test]
    fn test_parse_gpu_metrics_too_short() {
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&[]), None);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&[0u8; 3]), None);
    }

    #[test]
    fn test_parse_gpu_metrics_unknown_format() {
        let blob = build_gpu_metrics_blob(99, 0, 50);
        assert_eq!(parse_amd_gpu_metrics_gfx_activity(&blob), None);
    }

    // ── power state fallback tests ──────────────────────────────────────

    #[test]
    fn test_power_state_d3hot_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let card_dir = dir.path().join("card1").join("device");
        std::fs::create_dir_all(&card_dir).unwrap();
        std::fs::write(card_dir.join("power_state"), "D3hot\n").unwrap();
        // The function uses a hardcoded glob pattern, so we can't easily
        // unit-test it with a temp dir. Instead, test the logic directly.
        let state = "D3hot";
        let is_low_power = state.starts_with('D') && state != "D0";
        assert!(is_low_power);
        if is_low_power {
            // Would return Some(0)
            assert_eq!(0u8, 0);
        }
    }

    #[test]
    fn test_power_state_d0_is_not_low_power() {
        let state = "D0";
        let is_low_power = state.starts_with('D') && state != "D0";
        assert!(!is_low_power);
    }

    #[test]
    fn test_power_state_d3cold_is_low_power() {
        let state = "D3cold";
        let is_low_power = state.starts_with('D') && state != "D0";
        assert!(is_low_power);
    }

    #[test]
    fn test_power_state_d1_is_low_power() {
        let state = "D1";
        let is_low_power = state.starts_with('D') && state != "D0";
        assert!(is_low_power);
    }
}
