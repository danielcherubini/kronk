use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GpuType {
    Cuda { version: String },
    Vulkan,
    Metal,
    RocM { version: String },
    CpuOnly,
    Custom,
}

#[derive(Debug, Clone)]
pub struct GpuCapability {
    pub gpu_type: GpuType,
    pub device_name: String,
    pub vram_mb: u64,
}

#[derive(Debug, Clone)]
pub struct SystemCapabilities {
    pub os: String,
    pub arch: String,
    pub gpu: Option<GpuCapability>,
    pub cmake_available: bool,
    pub compiler_available: bool,
    pub git_available: bool,
}

/// VRAM usage in MiB.
#[derive(Debug, Clone)]
pub struct VramInfo {
    /// Currently used VRAM in MiB
    pub used_mib: u64,
    /// Total VRAM in MiB
    pub total_mib: u64,
}

impl VramInfo {
    /// Available VRAM in MiB
    pub fn available_mib(&self) -> u64 {
        self.total_mib.saturating_sub(self.used_mib)
    }

    /// Available VRAM in bytes
    pub fn available_bytes(&self) -> u64 {
        self.available_mib() * 1024 * 1024
    }

    /// Total VRAM in bytes
    pub fn total_bytes(&self) -> u64 {
        self.total_mib * 1024 * 1024
    }
}

/// Detect GPU type and capabilities.
///
/// Checks nvidia-smi first (CUDA), then ROCm (AMD), then Vulkan (Intel/AMD),
/// then Metal (macOS), then falls back.
/// CUDA version is read from nvidia-smi header output (does NOT require nvcc).
pub fn detect_gpu() -> Option<GpuCapability> {
    if let Some(cuda) = detect_cuda_gpu() {
        return Some(cuda);
    }
    if let Some(rocm) = detect_rocm() {
        return Some(rocm);
    }
    if let Some(vulkan) = detect_vulkan_gpu() {
        return Some(vulkan);
    }
    if let Some(metal) = detect_metal_gpu() {
        return Some(metal);
    }
    None
}

fn detect_cuda_gpu() -> Option<GpuCapability> {
    // Get device name and VRAM
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total", "--format=csv,noheader"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.trim().split(", ").collect();
    if parts.len() < 2 {
        return None;
    }

    let device_name = parts[0].trim().to_string();
    let vram_str = parts[1].trim().replace(" MiB", "");
    let vram_mb: u64 = vram_str.parse().ok()?;

    // Get CUDA version from nvidia-smi header (NOT nvcc -- users often
    // have the driver but not the CUDA toolkit installed).
    let version = detect_cuda_version_from_smi().unwrap_or_else(|| "unknown".to_string());

    Some(GpuCapability {
        gpu_type: GpuType::Cuda { version },
        device_name,
        vram_mb,
    })
}

/// Parse CUDA version from `nvidia-smi` header output.
/// The header contains a line like: "CUDA Version: 12.4"
fn detect_cuda_version_from_smi() -> Option<String> {
    let output = std::process::Command::new("nvidia-smi").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Look for "CUDA Version: X.Y" in the table header
    for line in stdout.lines() {
        if let Some(idx) = line.find("CUDA Version:") {
            let after = &line[idx + "CUDA Version:".len()..];
            let version = after.trim().split_whitespace().next()?;
            return Some(version.to_string());
        }
    }
    None
}

/// Detect ROCm GPU (AMD).
/// Uses `rocminfo` to detect ROCm and parse version from header.
fn detect_rocm() -> Option<GpuCapability> {
    // Check if rocminfo is available
    let output = std::process::Command::new("rocminfo").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse device info from rocminfo output
    // Look for "Name" and "Total Amount of Memory" in the device table
    let device_name = stdout
        .lines()
        .find(|line| line.contains("Name"))
        .and_then(|line| line.split(':').nth(1))
        .map(|part| part.trim().to_string())?;

    // Parse memory (look for "Total Amount of Memory")
    let vram_str = stdout
        .lines()
        .find(|line| line.contains("Total Amount of Memory"))
        .and_then(|line| line.split(':').nth(1))
        .and_then(|s| s.trim().strip_suffix(" MiB"))
        .and_then(|s| s.parse::<u64>().ok())?;

    // Extract ROCm version from header (e.g., "ROCm Version: 6.1.2")
    let version = stdout
        .lines()
        .find(|line| line.contains("ROCm Version:"))
        .and_then(|line| line.split(':').nth(1))
        .map(|part| part.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    Some(GpuCapability {
        gpu_type: GpuType::RocM { version },
        device_name,
        vram_mb: vram_str,
    })
}

/// Detect Vulkan GPU (Intel/AMD on Linux, NVIDIA without CUDA).
/// Uses `vulkaninfo --summary` to detect Vulkan-capable GPUs.
fn detect_vulkan_gpu() -> Option<GpuCapability> {
    // Check if vulkaninfo is available
    let output = std::process::Command::new("vulkaninfo")
        .args(["--summary"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse device info from vulkaninfo output
    // Look for "Name" and "Memory Heap" or "Total Memory"
    let device_name = stdout
        .lines()
        .find(|line| line.contains("Name"))
        .and_then(|line| line.split(':').nth(1))
        .map(|part| part.trim().to_string())?;

    // Parse memory (look for various memory indicators)
    let vram_str = stdout
        .lines()
        .find(|line| line.contains("Total Memory"))
        .and_then(|line| line.split(':').nth(1))
        .map(|s| s.trim())
        .and_then(|s| {
            s.strip_suffix(" MiB")
                .or_else(|| s.strip_suffix(" GB"))
                .and_then(|s| s.parse::<u64>().ok())
        })?;

    Some(GpuCapability {
        gpu_type: GpuType::Vulkan,
        device_name,
        vram_mb: vram_str,
    })
}

/// Detect Metal GPU (Apple Silicon on macOS).
/// Uses `system_profiler` to detect Metal-capable GPUs.
#[cfg(target_os = "macos")]
fn detect_metal_gpu() -> Option<GpuCapability> {
    // Check if system_profiler is available
    let output = std::process::Command::new("system_profiler")
        .args(["SPDisplaysDataType"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse device info from system_profiler output
    // Look for "Chip" and "Memory"
    let device_name = stdout
        .lines()
        .find(|line| line.contains("Chip"))
        .and_then(|line| line.split(':').nth(1))
        .map(|part| part.trim().to_string())?;

    // Parse memory (look for "Memory" or "VRAM")
    let vram_str = stdout
        .lines()
        .find(|line| line.contains("Memory"))
        .and_then(|line| line.split(':').nth(1))
        .and_then(|s| s.strip_suffix(" MB"))
        .and_then(|s| s.parse::<u64>().ok())?;

    Some(GpuCapability {
        gpu_type: GpuType::Metal,
        device_name,
        vram_mb: vram_str,
    })
}

/// Detect Metal GPU (Apple Silicon on macOS).
/// Uses `system_profiler` to detect Metal-capable GPUs.
#[cfg(not(target_os = "macos"))]
fn detect_metal_gpu() -> Option<GpuCapability> {
    None
}

pub fn detect_system_capabilities() -> SystemCapabilities {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let gpu = detect_gpu();

    let cmake_available = std::process::Command::new("cmake")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let git_available = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let compiler_available = {
        #[cfg(target_os = "windows")]
        {
            // Try MSVC (cl.exe) first, then MinGW (g++)
            std::process::Command::new("cl.exe")
                .arg("/?")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
                || std::process::Command::new("g++")
                    .arg("--version")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Try g++ first (C++ compiler needed for llama.cpp), then c++
            std::process::Command::new("g++")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
                || std::process::Command::new("c++")
                    .arg("--version")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
        }
    };

    SystemCapabilities {
        os,
        arch,
        gpu,
        cmake_available,
        compiler_available,
        git_available,
    }
}

/// Query GPU VRAM via nvidia-smi. Returns None if no NVIDIA GPU or nvidia-smi unavailable.
pub fn query_vram() -> Option<VramInfo> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.trim().split(", ").collect();
    if parts.len() == 2 {
        let used = parts[0].trim().parse().ok()?;
        let total = parts[1].trim().parse().ok()?;
        Some(VramInfo {
            used_mib: used,
            total_mib: total,
        })
    } else {
        None
    }
}

/// A suggested context size with metadata.
#[derive(Debug, Clone)]
pub struct ContextSuggestion {
    /// Context length in tokens
    pub context_length: u32,
    /// Human-readable label
    pub label: String,
    /// Whether this fits in available VRAM
    pub fits: bool,
    /// Estimated VRAM needed for KV cache at this context (MiB)
    pub kv_cache_mib: u64,
}

/// Suggest context sizes based on available VRAM and model size.
///
/// Uses a simple heuristic: KV cache grows roughly linearly with context.
/// For a ~7-9B model, KV cache at FP16 is ~0.5 GiB per 8K context.
/// With quantized KV (Q4/Q8), it's 2-4x smaller.
///
/// `model_size_bytes`: size of the GGUF file on disk
/// `vram`: GPU VRAM info (if available)
pub fn suggest_context_sizes(
    model_size_bytes: u64,
    vram: Option<&VramInfo>,
) -> Vec<ContextSuggestion> {
    // Estimate param count from model size (very rough)
    // Q4_K_M: ~0.6 bytes/param, Q8_0: ~1.1 bytes/param, FP16: ~2 bytes/param
    // Use 0.8 as middle ground for "average quant"
    let est_params_b = model_size_bytes as f64 / 0.8 / 1_000_000_000.0;

    // KV cache sizing (empirical from llama.cpp benchmarks):
    // A 7B model with FP16 KV cache uses ~256 MiB per 4K context.
    // That's ~64 MiB per 1K context for 7B params.
    // With Q4_0 KV cache it's ~4x less, but we estimate for FP16 (safe default).
    let mib_per_1k_ctx = 64.0 * (est_params_b / 7.0);

    let context_tiers: Vec<(u32, &str)> = vec![
        (2048, "2K (minimal)"),
        (4096, "4K (small)"),
        (8192, "8K (standard)"),
        (16384, "16K"),
        (32768, "32K"),
        (65536, "64K"),
        (100000, "100K"),
        (131072, "128K (max for most models)"),
    ];

    let available_for_kv = match vram {
        Some(v) => {
            let model_mib = model_size_bytes / (1024 * 1024);
            // Reserve model size + 512 MiB overhead for compute buffers
            v.total_mib.saturating_sub(model_mib).saturating_sub(512)
        }
        None => u64::MAX, // No GPU info — mark everything as "fits" but we don't know
    };

    context_tiers
        .into_iter()
        .map(|(ctx, desc)| {
            let kv_mib = (mib_per_1k_ctx * (ctx as f64 / 1024.0)) as u64;
            let fits = kv_mib <= available_for_kv;
            ContextSuggestion {
                context_length: ctx,
                label: format!(
                    "{} — ~{} MiB KV cache{}",
                    desc,
                    kv_mib,
                    if fits { "" } else { "  [may not fit]" }
                ),
                fits,
                kv_cache_mib: kv_mib,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vram_info_available() {
        let info = VramInfo {
            used_mib: 2000,
            total_mib: 8000,
        };
        assert_eq!(info.available_mib(), 6000);
    }

    #[test]
    fn test_suggest_context_sizes_no_gpu() {
        // 5.6 GB model, no GPU info
        let suggestions = suggest_context_sizes(5_600_000_000, None);
        assert!(!suggestions.is_empty());
        // All should be marked as fits since we don't know
        assert!(suggestions.iter().all(|s| s.fits));
    }

    #[test]
    fn test_suggest_context_sizes_with_gpu() {
        let vram = VramInfo {
            used_mib: 0,
            total_mib: 8192, // 8 GiB GPU
        };
        // 5.6 GB Q4 model — leaves ~2.3 GiB for KV after model + overhead
        let suggestions = suggest_context_sizes(5_600_000_000, Some(&vram));
        assert!(!suggestions.is_empty());
        // Small contexts should fit
        assert!(suggestions[0].fits); // 2K
        assert!(suggestions[1].fits); // 4K
                                      // 32K just barely fits (2048 MiB KV < 2339 MiB available)
        let ctx_32k = suggestions
            .iter()
            .find(|s| s.context_length == 32768)
            .unwrap();
        assert!(ctx_32k.fits);
        // 64K+ should not fit (~4 GiB KV > 2.3 GiB available)
        let ctx_64k = suggestions
            .iter()
            .find(|s| s.context_length == 65536)
            .unwrap();
        assert!(!ctx_64k.fits);
        // 128K definitely won't fit
        let last = suggestions.last().unwrap();
        assert!(!last.fits);
    }

    #[test]
    fn test_suggest_context_sizes_large_gpu() {
        let vram = VramInfo {
            used_mib: 0,
            total_mib: 24576, // 24 GiB GPU
        };
        // Small model on big GPU — more contexts should fit
        let suggestions = suggest_context_sizes(5_600_000_000, Some(&vram));
        let fitting: Vec<_> = suggestions.iter().filter(|s| s.fits).collect();
        // 24 GiB - 5.3 GiB model - 0.5 GiB overhead = ~18 GiB for KV
        // 8K = ~512 MiB, 16K = ~1GiB, 32K = ~2GiB — all should fit
        assert!(fitting.len() >= 4);
    }

    #[test]
    fn test_detect_system_capabilities() {
        let caps = detect_system_capabilities();
        assert!(!caps.os.is_empty());
        assert!(!caps.arch.is_empty());
    }

    #[test]
    fn test_gpu_type_display() {
        let cuda = GpuType::Cuda {
            version: "12.4".to_string(),
        };
        assert!(format!("{:?}", cuda).contains("12.4"));
    }
}
