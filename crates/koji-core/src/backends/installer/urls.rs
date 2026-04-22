use anyhow::{anyhow, Result};

use super::super::registry::BackendType;
use crate::gpu::GpuType;

/// Supported CUDA versions for prebuilt binaries.
///
/// This constant is the single source of truth for CUDA version mapping.
/// The UI/API should use this same constant to populate version dropdowns.
pub const SUPPORTED_CUDA_VERSIONS: &[&str] = &["11.1", "12.4", "13.1"];

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
                    let cuda_ver = map_cuda_version(version.as_str())?;
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

/// Maps a CUDA version string to a supported version.
///
/// This function is the single source of truth for CUDA version mapping.
/// It maps various user-input versions to the supported versions in SUPPORTED_CUDA_VERSIONS.
fn map_cuda_version(version: &str) -> Result<&'static str> {
    // First check if it's already a supported version
    if let Some(&supported) = SUPPORTED_CUDA_VERSIONS.iter().find(|&&v| v == version) {
        return Ok(supported);
    }

    // Map common version prefixes to supported versions
    let normalized = version.trim_start_matches('v');
    if let Some(dot_pos) = normalized.find('.') {
        let major = &normalized[..dot_pos];
        let supported = match major {
            "11" => "11.1",
            "12" => "12.4",
            "13" => "13.1",
            _ => {
                return Err(anyhow!(
                    "Unsupported CUDA version '{}'. Supported: {}.\n\
                     llama.cpp pre-built binaries only ship with CUDA {}.",
                    version,
                    SUPPORTED_CUDA_VERSIONS
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    SUPPORTED_CUDA_VERSIONS
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        };
        return Ok(supported);
    }

    Err(anyhow!(
        "Unsupported CUDA version '{}'. Supported: {}.\n\
         llama.cpp pre-built binaries only ship with CUDA {}.",
        version,
        SUPPORTED_CUDA_VERSIONS
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        SUPPORTED_CUDA_VERSIONS
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
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
    fn test_llama_cpp_download_url_windows_cuda13() {
        let url = get_prebuilt_url(
            &BackendType::LlamaCpp,
            "b8407",
            "windows",
            "x86_64",
            Some(&GpuType::Cuda {
                version: "13.4".to_string(),
            }),
        )
        .unwrap();

        assert!(url.contains("cuda-13.1"));
        assert!(url.contains("b8407"));
    }

    #[test]
    fn test_ik_llama_prebuilt_not_available() {
        let result = get_prebuilt_url(&BackendType::IkLlama, "main", "linux", "x86_64", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_supported_cuda_versions_all_map_to_urls() {
        // Assert that every version in SUPPORTED_CUDA_VERSIONS produces a valid URL
        for cuda_ver in SUPPORTED_CUDA_VERSIONS {
            let url = get_prebuilt_url(
                &BackendType::LlamaCpp,
                "b8407",
                "windows",
                "x86_64",
                Some(&GpuType::Cuda {
                    version: cuda_ver.to_string(),
                }),
            )
            .unwrap_or_else(|_| panic!("CUDA version {} should produce a valid URL", cuda_ver));
            assert!(
                url.contains(&format!("cuda-{}", cuda_ver)),
                "URL for CUDA version {} should contain 'cuda-{}', got: {}",
                cuda_ver,
                cuda_ver,
                url
            );
        }
    }

    #[test]
    fn test_map_cuda_version_supported_versions() {
        for supported in SUPPORTED_CUDA_VERSIONS {
            assert_eq!(
                map_cuda_version(supported).unwrap(),
                *supported,
                "Supported version {} should map to itself",
                supported
            );
        }
    }

    #[test]
    fn test_map_cuda_version_prefix_mapping() {
        // Test major version prefixes
        assert_eq!(map_cuda_version("11.0").unwrap(), "11.1");
        assert_eq!(map_cuda_version("11.5").unwrap(), "11.1");
        assert_eq!(map_cuda_version("12.0").unwrap(), "12.4");
        assert_eq!(map_cuda_version("12.6").unwrap(), "12.4");
        assert_eq!(map_cuda_version("13.0").unwrap(), "13.1");
        assert_eq!(map_cuda_version("13.7").unwrap(), "13.1");
    }

    #[test]
    fn test_map_cuda_version_invalid() {
        let result = map_cuda_version("10.0");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unsupported CUDA version"));
    }
}
