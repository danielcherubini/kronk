use anyhow::{anyhow, Result};

use super::super::registry::BackendType;
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
                // Windows
                ("windows", "x86_64", Some(GpuType::Cuda { ref version })) => {
                    let cuda_ver = match version.as_str() {
                        "11" | "11.0" | "11.1" | "11.2" | "11.3" | "11.4" | "11.5" | "11.6" | "11.7" | "11.8" => "11.1",
                        "12" | "12.0" | "12.1" | "12.2" | "12.3" | "12.4" | "12.5" | "12.6" => "12.4",
                        "13" | "13.0" | "13.1" | "13.2" | "13.3" | "13.4" | "13.5" | "13.6" | "13.7" => "13.1",
                        _ => {
                            return Err(anyhow!(
                                "Unsupported CUDA version '{}'. Supported: 11.x, 12.x, 13.x.\n\
                                 llama.cpp pre-built binaries only ship with CUDA 11.1, 12.4, and 13.1.",
                                version
                            ));
                        }
                    };
                    format!("llama-{}-bin-win-cuda-{}-x64.zip", tag, cuda_ver)
                }
                ("windows", "x86_64", Some(GpuType::Vulkan)) => {
                    format!("llama-{}-bin-win-vulkan-x64.zip", tag)
                }
                ("windows", "x86_64", _) => {
                    format!("llama-{}-bin-win-cpu-x64.zip", tag)
                }
                ("windows", "aarch64", _) => {
                    format!("llama-{}-bin-win-cpu-arm64.zip", tag)
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
        BackendType::Custom => {
            Err(anyhow!("Custom backends must be added manually"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::GpuType;

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
    fn test_llama_cpp_download_url_windows_cuda() {
        let url = get_prebuilt_url(
            &BackendType::LlamaCpp,
            "b8407",
            "windows",
            "x86_64",
            Some(&GpuType::Cuda {
                version: "12.4".to_string(),
            }),
        )
        .unwrap();

        assert!(url.contains("cuda-12.4"));
        assert!(url.contains("b8407"));
    }

    #[test]
    fn test_llama_cpp_download_url_windows_vulkan() {
        let url = get_prebuilt_url(
            &BackendType::LlamaCpp,
            "b8407",
            "windows",
            "x86_64",
            Some(&GpuType::Vulkan),
        )
        .unwrap();

        assert!(url.contains("vulkan"));
    }

    #[test]
    fn test_ik_llama_prebuilt_not_available() {
        let result = get_prebuilt_url(&BackendType::IkLlama, "main", "linux", "x86_64", None);
        assert!(result.is_err());
    }
}
