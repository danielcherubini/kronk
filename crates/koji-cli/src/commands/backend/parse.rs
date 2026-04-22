use anyhow::{anyhow, Result};
use koji_core::backends::BackendType;
use koji_core::config::Config;
use koji_core::gpu;

/// Parse a backend type string into a BackendType enum.
pub(crate) fn parse_backend_type(s: &str) -> Result<BackendType> {
    match s.to_lowercase().as_str() {
        "llama_cpp" | "llama.cpp" | "llamacpp" => Ok(BackendType::LlamaCpp),
        "ik_llama" | "ik-llama" | "ikllama" | "ik_llama.cpp" => Ok(BackendType::IkLlama),
        "tts_kokoro" | "ttskokoro" | "kokoro" => Ok(BackendType::TtsKokoro),
        _ => Err(anyhow!(
            "Unknown backend type '{}'. Supported: llama_cpp, ik_llama, tts_kokoro",
            s
        )),
    }
}

/// Parse a GPU type string into a GpuType enum.
pub(crate) fn parse_gpu_type(gpu_str: &str) -> Result<gpu::GpuType> {
    let gpu_str = gpu_str.trim().to_lowercase();

    match gpu_str.as_str() {
        "cpu" => Ok(gpu::GpuType::CpuOnly),
        "cuda" => {
            let version = gpu::detect_cuda_version()
                .unwrap_or_else(|| {
                    eprintln!(
                        "Warning: Could not auto-detect CUDA version (nvcc/nvidia-smi not found). \
                         Defaulting to {}. Use 'cuda:<version>' to specify explicitly.",
                        gpu::DEFAULT_CUDA_VERSION
                    );
                    gpu::DEFAULT_CUDA_VERSION.to_string()
                });
            println!("Detected CUDA version: {}", version);
            Ok(gpu::GpuType::Cuda { version })
        }
        "rocm" => Ok(gpu::GpuType::RocM {
            version: "6.1".to_string(),
        }),
        "vulkan" => Ok(gpu::GpuType::Vulkan),
        "metal" => Ok(gpu::GpuType::Metal),
        s if s.starts_with("cuda:") => {
            let version = s.strip_prefix("cuda:").unwrap();
            if version.is_empty() {
                anyhow::bail!("Invalid --gpu value: missing CUDA version after 'cuda:'");
            }
            Ok(gpu::GpuType::Cuda {
                version: version.to_string(),
            })
        }
        s if s.starts_with("rocm:") => {
            let version = s.strip_prefix("rocm:").unwrap();
            if version.is_empty() {
                anyhow::bail!("Invalid --gpu value: missing ROCm version after 'rocm:'");
            }
            Ok(gpu::GpuType::RocM {
                version: version.to_string(),
            })
        }
        _ => anyhow::bail!(
            "Unknown GPU type '{}'. Supported: cpu, cuda, cuda:<version>, rocm, rocm:<version>, vulkan, metal",
            gpu_str
        ),
    }
}

/// Get the registry config directory from the base config path.
pub(crate) fn registry_config_dir() -> Result<std::path::PathBuf> {
    Config::base_dir()
}

/// Get the current Unix timestamp in seconds.
pub(crate) fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use koji_core::backends::BackendType;

    // ── parse_backend_type tests ──────────────────────────────────────────

    #[test]
    fn test_parse_backend_type_llama_cpp() {
        let result = parse_backend_type("llama_cpp").unwrap();
        assert!(matches!(result, BackendType::LlamaCpp));
    }

    #[test]
    fn test_parse_backend_type_llama_dot_cpp() {
        let result = parse_backend_type("llama.cpp").unwrap();
        assert!(matches!(result, BackendType::LlamaCpp));
    }

    #[test]
    fn test_parse_backend_type_llamacpp() {
        let result = parse_backend_type("llamacpp").unwrap();
        assert!(matches!(result, BackendType::LlamaCpp));
    }

    #[test]
    fn test_parse_backend_type_ik_llama() {
        let result = parse_backend_type("ik_llama").unwrap();
        assert!(matches!(result, BackendType::IkLlama));
    }

    #[test]
    fn test_parse_backend_type_ik_llama_dash() {
        let result = parse_backend_type("ik-llama").unwrap();
        assert!(matches!(result, BackendType::IkLlama));
    }

    #[test]
    fn test_parse_backend_type_ikllama() {
        let result = parse_backend_type("ikllama").unwrap();
        assert!(matches!(result, BackendType::IkLlama));
    }

    #[test]
    fn test_parse_backend_type_ik_llama_dot_cpp() {
        let result = parse_backend_type("ik_llama.cpp").unwrap();
        assert!(matches!(result, BackendType::IkLlama));
    }

    #[test]
    fn test_parse_backend_type_case_insensitive() {
        let llama_result = parse_backend_type("LLAMA_CPP").unwrap();
        assert!(matches!(llama_result, BackendType::LlamaCpp));

        let ik_result = parse_backend_type("IK_LLAMA").unwrap();
        assert!(matches!(ik_result, BackendType::IkLlama));

        let mixed = parse_backend_type("Llama.Cpp").unwrap();
        assert!(matches!(mixed, BackendType::LlamaCpp));
    }

    #[test]
    fn test_parse_backend_type_unknown() {
        let result = parse_backend_type("unknown_backend");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown backend type"));
        assert!(err.contains("llama_cpp"));
        assert!(err.contains("ik_llama"));
        assert!(err.contains("tts_kokoro"));
    }

    #[test]
    fn test_parse_backend_type_empty() {
        let result = parse_backend_type("");
        assert!(result.is_err());
    }

    // ── TTS backend type tests ────────────────────────────────────────────

    #[test]
    fn test_parse_backend_type_tts_kokoro() {
        let result = parse_backend_type("tts_kokoro").unwrap();
        assert!(matches!(result, BackendType::TtsKokoro));
    }

    #[test]
    fn test_parse_backend_type_tts_kokoro_no_dash() {
        let result = parse_backend_type("ttskokoro").unwrap();
        assert!(matches!(result, BackendType::TtsKokoro));
    }

    #[test]
    fn test_parse_backend_type_tts_kokoro_short() {
        let result = parse_backend_type("kokoro").unwrap();
        assert!(matches!(result, BackendType::TtsKokoro));
    }

    #[test]
    fn test_parse_backend_type_tts_case_insensitive() {
        let kokoro_result = parse_backend_type("TTS_KOKORO").unwrap();
        assert!(matches!(kokoro_result, BackendType::TtsKokoro));
    }

    #[test]
    fn test_parse_backend_type_unknown_tts_error_message() {
        let result = parse_backend_type("unknown_tts");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown backend type"));
        assert!(err.contains("tts_kokoro"));
    }

    // ── current_unix_timestamp tests ──────────────────────────────────────

    #[test]
    fn test_current_unix_timestamp_positive() {
        let ts = current_unix_timestamp();
        // Should be a reasonable Unix timestamp (after year 2020)
        assert!(ts > 1_577_836_800);
        // Should not be in the future by more than a minute
        assert!(ts < 2_000_000_000);
    }
}
