//! Discovery helpers for the llama-bench binary and GPU-type inference.
//!
//! Kept separate from the orchestrator because they're pure filesystem /
//! path-string logic with no runtime dependencies, which makes them easy to
//! test in isolation.

use anyhow::{bail, Result};
use std::path::PathBuf;

/// Locate the llama-bench binary.
/// Search order:
/// 1. `LLAMA_BENCH_PATH` environment variable
/// 2. Same directory as the backend binary (e.g. `~/.config/koji/backends/.../llama-bench`)
/// 3. Grandparent's `tools/` directory (llama.cpp source tree layout)
/// 4. `PATH` lookup (system install)
pub fn find_llama_bench(backend_path: &std::path::Path) -> Result<PathBuf> {
    if let Ok(p) = std::env::var("LLAMA_BENCH_PATH") {
        let p = std::path::PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }

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

    for path_dir in std::env::split_paths(&std::env::var("PATH").unwrap_or_default()) {
        let candidate = path_dir.join(bench_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    bail!(
        "llama-bench binary not found. Install llama.cpp from source or set LLAMA_BENCH_PATH env var."
    )
}

/// Detect GPU type from backend binary path.
///
/// Matches substrings first (cheap, no GPU probing). Anything unknown reports
/// as "CPU" — we avoid live VRAM probing because it can misidentify the backend
/// (e.g. a ROCm build running on a system that also has a CUDA card).
pub(super) fn detect_gpu_type(backend_path: &std::path::Path) -> String {
    let path_lower = backend_path.to_string_lossy().to_lowercase();
    if path_lower.contains("vulkan") {
        "Vulkan".to_string()
    } else if path_lower.contains("cuda") {
        "CUDA".to_string()
    } else if path_lower.contains("rocm") || path_lower.contains("hip") {
        "ROCm".to_string()
    } else if path_lower.contains("metal") {
        "Metal".to_string()
    } else {
        "CPU".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `find_llama_bench` returns an error when no binary is found.
    #[test]
    fn test_find_llama_bench_not_found() {
        let nonexistent = std::path::PathBuf::from("/nonexistent/path/llama-server");
        let result = find_llama_bench(&nonexistent);
        assert!(result.is_err());
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
