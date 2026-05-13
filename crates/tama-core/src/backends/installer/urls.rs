use anyhow::{anyhow, Result};

use crate::backends::types::BackendType;
use crate::gpu::GpuType;

/// Construct the GitHub release download URL for a pre-built binary.
///
/// Note: `gpu` is taken by reference to avoid ownership issues.
/// Note: The `tag` parameter is the release tag (e.g. "b8407"), kept
/// separate from any GPU version strings to avoid shadowing.
pub fn get_prebuilt_url(
    backend: &BackendType,
    tag: &str,
    os: &str,
    arch: &str,
    gpu: Option<&GpuType>,
) -> Result<String> {
    match backend {
        BackendType::LlamaCpp => {
            let base = format!(
                "https://github.com/ggml-org/llama.cpp/releases/download/{}/",
                tag
            );

            let filename = match (os, arch, gpu) {
                // Linux
                ("linux", "x86_64", Some(GpuType::Vulkan)) => {
                    format!("llama-{}-bin-ubuntu-vulkan-x64.tar.gz", tag)
                }
                ("linux", "x86_64", Some(GpuType::RocM { .. })) => {
                    format!("llama-{}-bin-ubuntu-rocm-7.2-x64.tar.gz", tag)
                }
                ("linux", "x86_64", _) => {
                    // CPU and CUDA both use the ubuntu-x64 build
                    // (llama.cpp doesn't ship Linux CUDA pre-built binaries)
                    format!("llama-{}-bin-ubuntu-x64.tar.gz", tag)
                }
                // macOS
                ("macos", "aarch64", _) => {
                    format!("llama-{}-bin-macos-arm64.tar.gz", tag)
                }
                ("macos", "x86_64", _) => {
                    format!("llama-{}-bin-macos-x64.tar.gz", tag)
                }
                _ => return Err(anyhow!("Unsupported platform: {} {}", os, arch)),
            };

            Ok(format!("{}{}", base, filename))
        }
        BackendType::IkLlama => {
            Err(anyhow!(
                "ik_llama does not provide pre-built release binaries. Use --build to build from source."
            ))
        }
        BackendType::TtsKokoro => {
            Err(anyhow!(
                "TTS backends do not provide pre-built release binaries. Use --build to build from source."
            ))
        }
        BackendType::Custom => {
            Err(anyhow!("Custom backends must be added manually"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llama_cpp_download_url_linux_cpu() {
        let url =
            get_prebuilt_url(&BackendType::LlamaCpp, "b8407", "linux", "x86_64", None).unwrap();

        assert_eq!(
            url,
            "https://github.com/ggml-org/llama.cpp/releases/download/b8407/llama-b8407-bin-ubuntu-x64.tar.gz"
        );
    }

    #[test]
    fn test_ik_llama_prebuilt_not_available() {
        let result = get_prebuilt_url(&BackendType::IkLlama, "main", "linux", "x86_64", None);
        assert!(result.is_err());
    }
}
