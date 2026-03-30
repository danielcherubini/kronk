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
pub struct BuildPrerequisites {
    pub os: String,
    pub arch: String,
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

pub fn detect_build_prerequisites() -> BuildPrerequisites {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();

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
            // Also check vswhere.exe to detect VS Build Tools/VS installation
            let cl_available = std::process::Command::new("cl.exe")
                .arg("/?")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            let gpp_available = std::process::Command::new("g++")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            // Check vswhere.exe for VS Build Tools
            let program_files = std::env::var("ProgramFiles(x86)")
                .unwrap_or_else(|_| "C:\\Program Files (x86)".to_string());
            let vswhere_path = format!(
                "{}/Microsoft Visual Studio/Installer/vswhere.exe",
                program_files
            );
            let vswhere_available = std::process::Command::new(&vswhere_path)
                .args(&[
                    "-latest",
                    "-products",
                    "*",
                    "-requires",
                    "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                    "-property",
                    "installationPath",
                ])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            cl_available || gpp_available || vswhere_available
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

    BuildPrerequisites {
        os,
        arch,
        cmake_available,
        compiler_available,
        git_available,
    }
}

/// Default CUDA version used when auto-detection fails.
pub const DEFAULT_CUDA_VERSION: &str = "12.4";

/// Detect the installed CUDA toolkit version.
///
/// Tries `nvcc --version` first (most reliable), then falls back to
/// `nvidia-smi` driver-reported CUDA version. Returns `None` if neither
/// is available.
pub fn detect_cuda_version() -> Option<String> {
    // Try nvcc first — this reports the actual toolkit version
    if let Some(ver) = detect_cuda_version_nvcc() {
        return Some(ver);
    }
    // Fall back to nvidia-smi — reports the max supported CUDA version
    detect_cuda_version_nvidia_smi()
}

/// Parse CUDA version from `nvcc --version` output.
fn detect_cuda_version_nvcc() -> Option<String> {
    let output = std::process::Command::new("nvcc")
        .arg("--version")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // nvcc output contains a line like: "Cuda compilation tools, release 12.4, V12.4.131"
    // or "Cuda compilation tools, release 13.1, V13.1.105"
    for line in stdout.lines() {
        if let Some(pos) = line.find("release ") {
            let after = &line[pos + 8..];
            // Take "12.4" from "12.4, V12.4.131"
            let version = after.split(',').next()?.trim();
            if !version.is_empty() {
                return Some(version.to_string());
            }
        }
    }
    None
}

/// Parse CUDA version from `nvidia-smi` output.
fn detect_cuda_version_nvidia_smi() -> Option<String> {
    let output = std::process::Command::new("nvidia-smi").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // nvidia-smi header contains: "CUDA Version: 12.4"
    for line in stdout.lines() {
        if let Some(pos) = line.find("CUDA Version:") {
            let after = &line[pos + 13..];
            let version = after.split_whitespace().next()?;
            if !version.is_empty() {
                return Some(version.to_string());
            }
        }
    }
    None
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
    fn test_detect_build_prerequisites() {
        let caps = detect_build_prerequisites();
        assert!(!caps.os.is_empty());
        assert!(!caps.arch.is_empty());
        // No gpu field — that's the point
    }

    #[test]
    fn test_detect_cuda_version_nvcc_parsing() {
        // Simulate nvcc output parsing
        let sample = "nvcc: NVIDIA (R) Cuda compiler driver\n\
                       Copyright (c) 2005-2024 NVIDIA Corporation\n\
                       Built on Thu_Mar_28_02:18:24_PDT_2024\n\
                       Cuda compilation tools, release 12.4, V12.4.131\n\
                       Build cuda_12.4.r12.4/compiler.34097967_0";

        let mut version = None;
        for line in sample.lines() {
            if let Some(pos) = line.find("release ") {
                let after = &line[pos + 8..];
                if let Some(v) = after.split(',').next() {
                    let v = v.trim();
                    if !v.is_empty() {
                        version = Some(v.to_string());
                    }
                }
            }
        }
        assert_eq!(version, Some("12.4".to_string()));
    }

    #[test]
    fn test_detect_cuda_version_nvcc_parsing_v13() {
        let sample = "Cuda compilation tools, release 13.1, V13.1.105";
        let mut version = None;
        for line in sample.lines() {
            if let Some(pos) = line.find("release ") {
                let after = &line[pos + 8..];
                if let Some(v) = after.split(',').next() {
                    let v = v.trim();
                    if !v.is_empty() {
                        version = Some(v.to_string());
                    }
                }
            }
        }
        assert_eq!(version, Some("13.1".to_string()));
    }

    #[test]
    fn test_detect_cuda_version_nvidia_smi_parsing() {
        let sample =
            "| NVIDIA-SMI 550.54.14    Driver Version: 550.54.14    CUDA Version: 12.4     |";
        let mut version = None;
        for line in sample.lines() {
            if let Some(pos) = line.find("CUDA Version:") {
                let after = &line[pos + 13..];
                if let Some(v) = after.trim().split_whitespace().next() {
                    if !v.is_empty() {
                        version = Some(v.to_string());
                    }
                }
            }
        }
        assert_eq!(version, Some("12.4".to_string()));
    }

    #[test]
    fn test_default_cuda_version_is_set() {
        assert!(!DEFAULT_CUDA_VERSION.is_empty());
    }
}
