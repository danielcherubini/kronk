use std::path::Path;
use std::sync::Arc;

use crate::backends::InstallOptions;
use crate::backends::ProgressSink;
use crate::gpu::GpuType;

/// Emit a log line through the progress sink, or println if no sink is provided.
pub(crate) fn emit(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>) {
    let line = line.into();
    match sink {
        Some(s) => s.log(&line),
        None => println!("{line}"),
    }
}

/// Build the CMake argument list for the configure step.
///
/// Extracted for testability — callers can verify flags without invoking cmake.
pub(crate) fn build_cmake_args(
    options: &InstallOptions,
    source_dir: &Path,
    build_output: &Path,
    amdgpu_targets: &[String],
) -> Vec<String> {
    let mut cmake_args = vec![
        "-B".to_string(),
        build_output.to_string_lossy().to_string(),
        "-S".to_string(),
        source_dir.to_string_lossy().to_string(),
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
    ];

    // Add GPU-specific flags
    if let Some(ref gpu) = options.gpu_type {
        match gpu {
            GpuType::Cuda { .. } => {
                cmake_args.push("-DGGML_CUDA=ON".to_string());
            }
            GpuType::Vulkan => {
                cmake_args.push("-DGGML_VULKAN=ON".to_string());
            }
            GpuType::Metal => {
                cmake_args.push("-DGGML_METAL=ON".to_string());
            }
            GpuType::RocM { .. } => {
                cmake_args.push("-DGGML_HIP=ON".to_string());
                cmake_args.push("-DGGML_HIP_ROCWMMA_FATTN=ON".to_string());
                cmake_args.push("-DGGML_CUDA_FA_ALL_QUANTS=ON".to_string());
                cmake_args.push("-DGGML_BACKEND_DL=ON".to_string());
                // Note: `-DLLAMA_CURL=ON` was deprecated upstream and is now
                // silently ignored (emits a cmake warning). curl support is
                // handled implicitly by current llama.cpp builds, so we do
                // not pass the flag.
                if !amdgpu_targets.is_empty() {
                    cmake_args.push(format!("-DAMDGPU_TARGETS={}", amdgpu_targets.join(";")));
                }
            }
            GpuType::CpuOnly => {}
            GpuType::Custom => {}
        }
    }

    // Explicitly enable all IQK FlashAttention KV cache quant types for ik_llama.
    // This defaults to ON in current ik_llama.cpp main, but we set it explicitly
    // to guard against any future default change. Without it, sub-q8_0 KV cache
    // types cause NaN crashes on hybrid Mamba/attention models (e.g. Qwen3.5).
    // Note: this is GGML_IQK_FA_ALL_QUANTS (CPU IQK kernels), distinct from
    // GGML_CUDA_FA_ALL_QUANTS (CUDA FlashAttention kernels, defaults OFF).
    if matches!(
        options.backend_type,
        crate::backends::types::BackendType::IkLlama
    ) {
        cmake_args.push("-DGGML_IQK_FA_ALL_QUANTS=ON".to_string());
    }

    cmake_args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::installer::source::detect;
    use crate::backends::types::{BackendSource, BackendType};
    use std::path::PathBuf;

    fn make_options(backend_type: BackendType, gpu_type: Option<GpuType>) -> InstallOptions {
        InstallOptions {
            backend_type,
            source: BackendSource::SourceCode {
                version: "main".to_string(),
                git_url: "https://example.com/repo.git".to_string(),
                commit: None,
            },
            target_dir: PathBuf::from("/tmp/test"),
            gpu_type,
            gpu_variant: "cpu".to_string(),
            allow_overwrite: false,
        }
    }

    /// ik_llama source builds must explicitly set GGML_IQK_FA_ALL_QUANTS=ON.
    /// It defaults to ON in current ik_llama.cpp main, but we set it explicitly
    /// to guard against any future default change. Without it, sub-q8_0 KV cache
    /// causes NaN crashes on hybrid Mamba/attention models (e.g. Qwen3.5).
    #[test]
    fn test_ik_llama_includes_iqk_fa_all_quants() {
        let opts = make_options(BackendType::IkLlama, None);
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(
            args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()),
            "ik_llama build must include -DGGML_IQK_FA_ALL_QUANTS=ON, got: {:?}",
            args
        );
    }

    /// llama.cpp builds must NOT include the ik_llama-specific flag.
    #[test]
    fn test_llama_cpp_excludes_iqk_fa_all_quants() {
        let opts = make_options(BackendType::LlamaCpp, None);
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(
            !args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()),
            "llama.cpp build must not include -DGGML_IQK_FA_ALL_QUANTS=ON"
        );
    }

    /// ik_llama + CUDA should have both the CUDA flag and the quants flag.
    #[test]
    fn test_ik_llama_cuda_includes_both_flags() {
        let opts = make_options(
            BackendType::IkLlama,
            Some(GpuType::Cuda {
                version: "12".to_string(),
            }),
        );
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(args.contains(&"-DGGML_CUDA=ON".to_string()));
        assert!(args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()));
    }

    /// ROCm source builds must emit the full ROCm flag set.
    #[test]
    fn test_rocm_emits_full_flag_set() {
        let opts = make_options(
            BackendType::LlamaCpp,
            Some(GpuType::RocM {
                version: "7.2".to_string(),
            }),
        );
        let args = build_cmake_args(
            &opts,
            Path::new("/src"),
            Path::new("/build"),
            &["gfx1201".to_string()],
        );
        assert!(
            args.contains(&"-DGGML_HIP=ON".to_string()),
            "ROCm build must include -DGGML_HIP=ON, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DGGML_HIP_ROCWMMA_FATTN=ON".to_string()),
            "ROCm build must include -DGGML_HIP_ROCWMMA_FATTN=ON, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DGGML_CUDA_FA_ALL_QUANTS=ON".to_string()),
            "ROCm build must include -DGGML_CUDA_FA_ALL_QUANTS=ON, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DGGML_BACKEND_DL=ON".to_string()),
            "ROCm build must include -DGGML_BACKEND_DL=ON, got: {:?}",
            args
        );
        assert!(
            !args.iter().any(|a| a.starts_with("-DLLAMA_CURL=")),
            "ROCm build must NOT include -DLLAMA_CURL= (deprecated upstream), got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DAMDGPU_TARGETS=gfx1201".to_string()),
            "ROCm build must include -DAMDGPU_TARGETS=gfx1201, got: {:?}",
            args
        );
    }

    /// Multiple AMDGPU targets are joined with semicolons (CMake list separator).
    #[test]
    fn test_rocm_multi_target_joined_with_semicolons() {
        let opts = make_options(
            BackendType::LlamaCpp,
            Some(GpuType::RocM {
                version: "7.2".to_string(),
            }),
        );
        let args = build_cmake_args(
            &opts,
            Path::new("/src"),
            Path::new("/build"),
            &["gfx1100".to_string(), "gfx1201".to_string()],
        );
        assert!(
            args.contains(&"-DAMDGPU_TARGETS=gfx1100;gfx1201".to_string()),
            "ROCm build must join targets with ';', got: {:?}",
            args
        );
    }

    /// When no AMDGPU targets are detected, the AMDGPU_TARGETS flag is omitted
    /// (fall back to llama.cpp's default list), but other ROCm flags remain.
    #[test]
    fn test_rocm_no_targets_omits_amdgpu_targets_flag() {
        let opts = make_options(
            BackendType::LlamaCpp,
            Some(GpuType::RocM {
                version: "7.2".to_string(),
            }),
        );
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(
            !args.iter().any(|a| a.starts_with("-DAMDGPU_TARGETS=")),
            "Empty targets must omit -DAMDGPU_TARGETS=, got: {:?}",
            args
        );
        assert!(args.contains(&"-DGGML_HIP=ON".to_string()));
        assert!(args.contains(&"-DGGML_HIP_ROCWMMA_FATTN=ON".to_string()));
        assert!(args.contains(&"-DGGML_CUDA_FA_ALL_QUANTS=ON".to_string()));
        assert!(args.contains(&"-DGGML_BACKEND_DL=ON".to_string()));
        assert!(
            !args.iter().any(|a| a.starts_with("-DLLAMA_CURL=")),
            "ROCm build must NOT include -DLLAMA_CURL= (deprecated upstream), got: {:?}",
            args
        );
    }

    /// Non-ROCm GPU types must never emit ROCm flags, even if amdgpu_targets
    /// is accidentally populated by the caller.
    #[test]
    fn test_non_rocm_never_emits_rocm_flags() {
        let opts = make_options(
            BackendType::LlamaCpp,
            Some(GpuType::Cuda {
                version: "12".to_string(),
            }),
        );
        let args = build_cmake_args(
            &opts,
            Path::new("/src"),
            Path::new("/build"),
            &["gfx1201".to_string()],
        );
        assert!(!args.contains(&"-DGGML_HIP=ON".to_string()));
        assert!(!args.contains(&"-DGGML_HIP_ROCWMMA_FATTN=ON".to_string()));
        assert!(
            !args.iter().any(|a| a.starts_with("-DAMDGPU_TARGETS=")),
            "non-ROCm build must not emit -DAMDGPU_TARGETS=, got: {:?}",
            args
        );
    }

    /// ik_llama + ROCm must include both the ik_llama-specific IQK flag and
    /// the ROCm-specific rocWMMA FlashAttention flag.
    #[test]
    fn test_ik_llama_rocm_includes_both_iqk_and_rocwmma() {
        let opts = make_options(
            BackendType::IkLlama,
            Some(GpuType::RocM {
                version: "7.2".to_string(),
            }),
        );
        let args = build_cmake_args(
            &opts,
            Path::new("/src"),
            Path::new("/build"),
            &["gfx942".to_string()],
        );
        assert!(args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()));
        assert!(args.contains(&"-DGGML_HIP_ROCWMMA_FATTN=ON".to_string()));
        assert!(args.contains(&"-DGGML_BACKEND_DL=ON".to_string()));
    }

    #[test]
    fn test_hip_env_from_hipconfig_output_happy_path() {
        let result = detect::hip_env_from_hipconfig_output("/opt/rocm/llvm/bin\n", "/opt/rocm\n");
        assert_eq!(
            result,
            Some((
                "/opt/rocm/llvm/bin/clang".to_string(),
                "/opt/rocm".to_string()
            ))
        );
    }

    #[test]
    fn test_hip_env_from_hipconfig_output_empty_stdout_returns_none() {
        assert_eq!(detect::hip_env_from_hipconfig_output("", "/opt/rocm"), None);
        assert_eq!(
            detect::hip_env_from_hipconfig_output("/opt/rocm/llvm/bin", "   "),
            None
        );
    }

    #[test]
    fn test_hip_env_from_hipconfig_output_trims_whitespace() {
        let result =
            detect::hip_env_from_hipconfig_output("  /opt/rocm/llvm/bin  \n", "\t/opt/rocm\t\n");
        assert_eq!(
            result,
            Some((
                "/opt/rocm/llvm/bin/clang".to_string(),
                "/opt/rocm".to_string()
            ))
        );
    }
}
