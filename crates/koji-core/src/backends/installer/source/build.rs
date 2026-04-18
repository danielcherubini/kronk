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
        super::super::super::registry::BackendType::IkLlama
    ) {
        cmake_args.push("-DGGML_IQK_FA_ALL_QUANTS=ON".to_string());

        // On Windows, use the Ninja + clang-cl approach recommended by the
        // ik_llama.cpp official build docs. This sidesteps all MSVC AVX2
        // detection issues: clang-cl accepts -march=native directly (via
        // /clang:-march=native), which reliably defines __AVX2__ and activates
        // the IQK optimized CPU kernels required by hybrid Mamba/attention
        // models like Qwen3.5. Without these kernels, SSM layers produce
        // inf/NaN logits and crash on the first token.
        if cfg!(target_os = "windows") {
            // Use Ninja generator so clang-cl works correctly (it doesn't
            // integrate well with the Visual Studio MSBuild generator).
            cmake_args.push("-GNinja".to_string());
            // clang-cl: LLVM's CL-compatible driver. Supports /arch:AVX2
            // reliably, unlike plain MSVC cl.exe where AVX2 detection is broken.
            cmake_args.push("-DCMAKE_C_COMPILER=clang-cl".to_string());
            cmake_args.push("-DCMAKE_CXX_COMPILER=clang-cl".to_string());
            // clang-cl identifies itself as MSVC to CMake, so ggml's CMakeLists
            // takes the MSVC branch for ARCH_FLAGS. In that branch:
            //   - GGML_NATIVE=ON  → runs FindSIMD.cmake which leaves ARCH_FLAGS
            //     empty for clang-cl (it only sets flags for true cl.exe)
            //   - GGML_AVX2=OFF   → skips /arch:AVX2 (default is OFF when NATIVE=ON)
            // Fix: disable NATIVE and explicitly enable AVX2/FMA/AVX so the MSVC
            // branch adds /arch:AVX2 to ARCH_FLAGS, which defines __AVX2__ and
            // activates the IQK optimized CPU kernels required by Qwen3.5/Mamba.
            cmake_args.push("-DGGML_NATIVE=OFF".to_string());
            cmake_args.push("-DGGML_AVX2=ON".to_string());
            cmake_args.push("-DGGML_AVX=ON".to_string());
            cmake_args.push("-DGGML_FMA=ON".to_string());
            // CUDA arch: "native" lets nvcc detect the installed GPU at compile
            // time. Required because without it CUDA 13.x would use the fallback
            // list which includes compute_50 (dropped in CUDA 13.x).
            cmake_args.push("-DCMAKE_CUDA_ARCHITECTURES=native".to_string());
        }
    }

    cmake_args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::installer::source::detect;
    use crate::backends::registry::{BackendSource, BackendType};
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
    }

    /// On Windows, ik_llama builds must use the Ninja + clang-cl approach so
    /// that -march=native is reliably passed via /clang:-march=native. This
    /// defines __AVX2__ and activates the IQK optimized CPU kernels required by
    /// hybrid Mamba/attention models (e.g. Qwen3.5). Without these kernels, SSM
    /// layers produce inf/NaN logits and crash on the first token.
    #[test]
    #[cfg(target_os = "windows")]
    fn test_ik_llama_windows_uses_ninja_clang_cl_avx2() {
        let opts = make_options(BackendType::IkLlama, None);
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"), &[]);
        assert!(
            args.contains(&"-GNinja".to_string()),
            "Windows ik_llama build must use -GNinja, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DCMAKE_C_COMPILER=clang-cl".to_string()),
            "Windows ik_llama build must set CMAKE_C_COMPILER=clang-cl, got: {:?}",
            args
        );
        // clang-cl acts as MSVC to CMake, so we must explicitly set AVX2/FMA/AVX
        // and disable NATIVE so ggml's MSVC branch adds /arch:AVX2 to ARCH_FLAGS.
        assert!(
            args.contains(&"-DGGML_NATIVE=OFF".to_string()),
            "Windows ik_llama build must set GGML_NATIVE=OFF, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DGGML_AVX2=ON".to_string()),
            "Windows ik_llama build must set GGML_AVX2=ON, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DGGML_FMA=ON".to_string()),
            "Windows ik_llama build must set GGML_FMA=ON, got: {:?}",
            args
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_hip_env_from_hipconfig_output_happy_path() {
        let result =
            detect::hip_env_from_hipconfig_output("/opt/rocm/llvm/bin\n", "/opt/rocm\n");
        assert_eq!(
            result,
            Some((
                "/opt/rocm/llvm/bin/clang".to_string(),
                "/opt/rocm".to_string()
            ))
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_hip_env_from_hipconfig_output_empty_stdout_returns_none() {
        assert_eq!(
            detect::hip_env_from_hipconfig_output("", "/opt/rocm"),
            None
        );
        assert_eq!(
            detect::hip_env_from_hipconfig_output("/opt/rocm/llvm/bin", "   "),
            None
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_hip_env_from_hipconfig_output_trims_whitespace() {
        let result = detect::hip_env_from_hipconfig_output(
            "  /opt/rocm/llvm/bin  \n",
            "\t/opt/rocm\t\n",
        );
        assert_eq!(
            result,
            Some((
                "/opt/rocm/llvm/bin/clang".to_string(),
                "/opt/rocm".to_string()
            ))
        );
    }
}
