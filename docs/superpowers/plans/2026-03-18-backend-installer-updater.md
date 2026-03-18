# Backend Installer/Updater Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add tooling to install and update llama.cpp and ik_llama backends, supporting both pre-built binaries and source builds.

**Architecture:** New `backend` subcommand with install/update/list/remove operations. Backends stored in managed `~/.config/kronk/backends/` directory. Auto-detection of GPU capabilities by extending the existing `gpu.rs` module with user confirmation. Support for both pre-built binaries (fast, simple) and source builds (flexible, hardware-specific). User chooses build method during install via interactive prompt.

**Tech Stack:** Rust, tokio (async), reqwest (downloads), sysinfo + nvidia-smi (GPU detection), indicatif (progress bars), flate2 + tar (pure-Rust archive extraction), CMake (source builds)

---

## Context

### Current State
- Kronk manages LLM servers but assumes backends (llama-server.exe) already exist
- Backends referenced by absolute paths in config.toml under `[backends.llama_cpp]`
- No automated installation or update mechanism
- Users must manually build/download backends
- An existing `gpu.rs` module (`crates/kronk-core/src/gpu.rs`) already queries nvidia-smi for VRAM info

### Target Backends
1. **llama.cpp** (https://github.com/ggml-org/llama.cpp, org: ggml-org)
   - Official GGML inference engine
   - Pre-built binaries available per release (b-series tags like b8407)
   - CMake build system
   - Platforms: Windows (x64, arm64), Linux (x64, aarch64), macOS (arm64, x64)
   - GPU: CUDA (12.4, 13.1), Vulkan, Metal, ROCm

2. **ik_llama** (https://github.com/ikawrakow/ik_llama.cpp)
   - Fork with better CPU performance + SOTA quants
   - Only one pre-release available (tag: t0002) -- **no stable releases**
   - `/releases/latest` API endpoint returns 404 (no non-prerelease exists)
   - CMake build system
   - Primarily CPU and CUDA support (Metal/Vulkan/ROCm are unsupported)
   - Platforms: Windows, Linux (focus on CPU-first builds)
   - **Must default to source builds** since pre-built binaries are not reliably available

### Design Decisions
- **Platforms:** Windows + Linux (match kronk's current support)
- **Build Strategy:** User chooses during install (pre-built or source); ik_llama only offers source
- **GPU Detection:** Extend existing `gpu.rs` module rather than creating a parallel detector
- **CLI Interface:** Separate `backend` subcommand (`kronk backend install/update/list/rm`)
- **Storage:** Managed `~/.config/kronk/backends/` + support for external paths
- **Version Management:** Track installed backends, check for updates
- **Timestamps:** Use i64 unix epoch (not SystemTime) for TOML serialization compatibility

---

## File Structure

### New Files to Create

```text
crates/kronk-core/src/
  backends/
    mod.rs              # Backend management module exports
    installer.rs        # Installation logic (download, build, verify)
    updater.rs          # Update checking and upgrading
    registry.rs         # Track installed backends and versions

crates/kronk-cli/src/
  commands/
    backend.rs          # Backend CLI subcommand handlers
```

### Files to Modify

```text
crates/kronk-core/src/
  lib.rs              # Export new backends module
  gpu.rs              # Extend with GPU type detection, system capabilities

crates/kronk-cli/src/
  main.rs             # Add Backend variant to Commands enum
  commands/mod.rs     # Add `pub mod backend;`

crates/kronk-core/Cargo.toml  # Add zip, flate2, tar dependencies
```

### Key Design Notes

- **No separate `detector.rs`**: The existing `gpu.rs` already handles nvidia-smi. We extend it rather than creating a parallel module. This avoids duplication.
- **No `num_cpus` crate**: Use `std::thread::available_parallelism()` (stable since Rust 1.59).
- **Registry uses `i64` timestamps**: `std::time::SystemTime` cannot be serialized by the `toml` crate. We use unix epoch seconds instead.
- **Integration tests live inside the crate** (`kronk-core`), not at the workspace root (no `tests/` dir exists at root).
- **Pure-Rust archive extraction**: Uses `flate2` + `tar` crates for .tar.gz and `zip` crate for .zip files. No shell-out to external `tar` command, which may not exist on all Windows machines.
- **Git prerequisite check**: `SystemCapabilities` includes `git_available` since source builds require `git clone`.

---

## Task 1: Extend GPU Module with Detection Capabilities

**Files:**
- Modify: `crates/kronk-core/src/gpu.rs`

This task extends the existing `gpu.rs` with GPU type classification and system capability detection. We reuse the existing `query_vram()` pattern and `nvidia-smi` approach.

- [ ] **Step 1: Write tests for new GPU detection types**

Add to the bottom of `crates/kronk-core/src/gpu.rs`, inside `mod tests`:

```rust
#[test]
fn test_detect_system_capabilities() {
    let caps = detect_system_capabilities();
    assert!(!caps.os.is_empty());
    assert!(!caps.arch.is_empty());
}

#[test]
fn test_gpu_type_display() {
    let cuda = GpuType::Cuda { version: "12.4".to_string() };
    assert!(format!("{:?}", cuda).contains("12.4"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package kronk-core --lib gpu::tests::test_detect_system_capabilities`
Expected: FAIL with "cannot find function/struct"

- [ ] **Step 3: Add GPU type enum and system capabilities**

Add to `crates/kronk-core/src/gpu.rs` (before the existing `impl VramInfo`):

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GpuType {
    Cuda { version: String },
    Vulkan,
    Metal,
    RocM { version: String },
    CpuOnly,
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

/// Detect GPU type and capabilities.
///
/// Checks nvidia-smi first (CUDA), then falls back.
/// CUDA version is read from nvidia-smi header output (does NOT require nvcc).
pub fn detect_gpu() -> Option<GpuCapability> {
    if let Some(cuda) = detect_cuda_gpu() {
        return Some(cuda);
    }
    // Future: detect Vulkan via vulkaninfo, Metal via system_profiler
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
    let output = std::process::Command::new("nvidia-smi")
        .output()
        .ok()?;

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
```

- [ ] **Step 4: Run tests**

Run: `cargo test --package kronk-core --lib gpu`
Expected: PASS (all existing tests + new tests)

- [ ] **Step 5: Commit GPU detection extensions**

```bash
git add crates/kronk-core/src/gpu.rs
git commit -m "feat(gpu): extend GPU module with type detection and system capabilities"
```

---

## Task 2: Backend Registry (Version Tracking)

**Files:**
- Create: `crates/kronk-core/src/backends/mod.rs`
- Create: `crates/kronk-core/src/backends/registry.rs`
- Modify: `crates/kronk-core/src/lib.rs`

- [ ] **Step 1: Write test for registry operations**

```rust
// In crates/kronk-core/src/backends/registry.rs
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_registry_add_and_list() {
        let tmp = TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.toml");
        let mut registry = BackendRegistry::load(&registry_path).unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        registry.add(BackendInfo {
            name: "llama_cpp".to_string(),
            backend_type: BackendType::LlamaCpp,
            version: "b8407".to_string(),
            path: "/path/to/llama-server".into(),
            installed_at: now,
            gpu_type: None,
        }).unwrap();

        let backends = registry.list();
        assert_eq!(backends.len(), 1);
        assert_eq!(backends[0].name, "llama_cpp");
    }

    #[test]
    fn test_registry_remove() {
        let tmp = TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.toml");
        let mut registry = BackendRegistry::load(&registry_path).unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        registry.add(BackendInfo {
            name: "llama_cpp".to_string(),
            backend_type: BackendType::LlamaCpp,
            version: "b8407".to_string(),
            path: "/path/to/llama-server".into(),
            installed_at: now,
            gpu_type: None,
        }).unwrap();

        registry.remove("llama_cpp").unwrap();
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_registry_roundtrip_serialization() {
        let tmp = TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.toml");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Write
        {
            let mut registry = BackendRegistry::load(&registry_path).unwrap();
            registry.add(BackendInfo {
                name: "test".to_string(),
                backend_type: BackendType::LlamaCpp,
                version: "b1234".to_string(),
                path: "/tmp/test".into(),
                installed_at: now,
                gpu_type: Some(crate::gpu::GpuType::Cuda { version: "12.4".to_string() }),
            }).unwrap();
        }

        // Read back
        let registry = BackendRegistry::load(&registry_path).unwrap();
        let backend = registry.get("test").unwrap();
        assert_eq!(backend.version, "b1234");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package kronk-core --lib backends::registry`
Expected: FAIL with "module not found"

- [ ] **Step 3: Create backends module structure**

```rust
// crates/kronk-core/src/backends/mod.rs
pub mod installer;
pub mod registry;
pub mod updater;

pub use installer::{install_backend, BackendSource, InstallOptions};
pub use registry::{BackendInfo, BackendRegistry, BackendType};
pub use updater::{check_latest_version, check_updates, update_backend, UpdateCheck};
```

- [ ] **Step 4: Implement backend registry**

```rust
// crates/kronk-core/src/backends/registry.rs
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::gpu::GpuType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackendType {
    LlamaCpp,
    IkLlama,
    Custom,
}

/// Metadata for an installed backend.
///
/// `installed_at` is a unix epoch timestamp (i64) because `SystemTime`
/// cannot be serialized by the `toml` crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub name: String,
    pub backend_type: BackendType,
    pub version: String,
    pub path: PathBuf,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_type: Option<GpuType>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct RegistryData {
    #[serde(default)]
    backends: HashMap<String, BackendInfo>,
}

pub struct BackendRegistry {
    path: PathBuf,
    data: RegistryData,
}

impl BackendRegistry {
    pub fn load(path: &Path) -> Result<Self> {
        let data = if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read registry at {:?}", path))?;
            toml::from_str(&content)
                .with_context(|| "Failed to parse registry")?
        } else {
            RegistryData::default()
        };

        Ok(Self {
            path: path.to_path_buf(),
            data,
        })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&self.data)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    pub fn add(&mut self, backend: BackendInfo) -> Result<()> {
        self.data.backends.insert(backend.name.clone(), backend);
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        self.data.backends.remove(name);
        self.save()
    }

    pub fn get(&self, name: &str) -> Option<&BackendInfo> {
        self.data.backends.get(name)
    }

    pub fn list(&self) -> Vec<&BackendInfo> {
        self.data.backends.values().collect()
    }

    pub fn update_version(&mut self, name: &str, new_version: String, new_path: PathBuf) -> Result<()> {
        if let Some(backend) = self.data.backends.get_mut(name) {
            backend.version = new_version;
            backend.path = new_path;
            self.save()?;
        }
        Ok(())
    }
}
```

- [ ] **Step 5: Export from lib.rs**

Add to `crates/kronk-core/src/lib.rs`:
```rust
pub mod backends;
```

- [ ] **Step 6: Run tests**

Run: `cargo test --package kronk-core --lib backends::registry`
Expected: PASS

- [ ] **Step 7: Commit registry**

```bash
git add crates/kronk-core/src/backends/ crates/kronk-core/src/lib.rs
git commit -m "feat(backends): add backend registry for version tracking"
```

---

## Task 3: Pre-built Binary Installer

**Files:**
- Create: `crates/kronk-core/src/backends/installer.rs`
- Modify: `crates/kronk-core/Cargo.toml`

- [ ] **Step 1: Write test for determining download URL**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::GpuType;

    #[test]
    fn test_llama_cpp_download_url_linux_cpu() {
        let url = get_prebuilt_url(
            &BackendType::LlamaCpp,
            "b8407",
            "linux",
            "x86_64",
            None,
        ).unwrap();

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
            Some(&GpuType::Cuda { version: "12.4".to_string() }),
        ).unwrap();

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
        ).unwrap();

        assert!(url.contains("vulkan"));
    }

    #[test]
    fn test_ik_llama_prebuilt_not_available() {
        let result = get_prebuilt_url(
            &BackendType::IkLlama,
            "main",
            "linux",
            "x86_64",
            None,
        );
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package kronk-core --lib backends::installer`
Expected: FAIL with "module not found"

- [ ] **Step 3: Implement installer**

```rust
// crates/kronk-core/src/backends/installer.rs
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

// Pure-Rust archive extraction (no external tar command needed)
use flate2::read::GzDecoder;

use crate::gpu::GpuType;
use super::registry::BackendType;

#[derive(Debug, Clone)]
pub enum BackendSource {
    Prebuilt { version: String },
    SourceCode { version: String, git_url: String },
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub backend_type: BackendType,
    pub source: BackendSource,
    pub target_dir: PathBuf,
    pub gpu_type: Option<GpuType>,
}

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
                    let cuda_ver = if version.starts_with("13") { "13.1" } else { "12.4" };
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

pub async fn download_file(url: &str, dest: &Path) -> Result<()> {
    let client = Client::builder()
        .user_agent("kronk-backend-manager")
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to download from {}", url))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Download failed with status: {}",
            response.status()
        ));
    }

    let total_size = response.content_length().unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut file = tokio::fs::File::create(dest).await?;
    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        pb.set_position(downloaded);
    }

    pb.finish_with_message("Download complete");
    Ok(())
}

/// Extract an archive (.zip or .tar.gz) to `dest` and return path to the llama-server binary.
///
/// Uses pure-Rust crates for extraction (flate2 + tar for .tar.gz, zip for .zip).
/// No external commands are required -- this works on any platform without tar in PATH.
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest)?;

    let filename = archive.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("Invalid archive path"))?;

    if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        let tar_file = std::fs::File::open(archive)
            .with_context(|| format!("Failed to open archive {:?}", archive))?;
        let gz = flate2::read::GzDecoder::new(tar_file);
        let mut tar_archive = tar::Archive::new(gz);
        tar_archive.unpack(dest)
            .with_context(|| "Failed to extract tar.gz archive")?;

        // Set executable permissions on extracted files (tar crate preserves
        // unix modes, but only if the archive contains them)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(entries) = std::fs::read_dir(dest) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Ok(meta) = path.metadata() {
                            let mode = meta.permissions().mode();
                            // If file has no execute bit, check if it's a binary
                            if mode & 0o111 == 0 {
                                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                    if name.starts_with("llama-") {
                                        let mut perms = meta.permissions();
                                        perms.set_mode(0o755);
                                        let _ = std::fs::set_permissions(&path, perms);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    } else if filename.ends_with(".zip") {
        let file = std::fs::File::open(archive)?;
        let mut zip = zip::ZipArchive::new(file)?;

        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let outpath = dest.join(entry.name());

            if entry.name().ends_with('/') {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    std::fs::create_dir_all(p)?;
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut entry, &mut outfile)?;
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = entry.unix_mode() {
                    std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(mode))?;
                }
            }
        }
    } else {
        return Err(anyhow!("Unsupported archive format: {}", filename));
    }

    find_backend_binary(dest)
}

/// Recursively search for the llama-server binary in the extracted directory.
fn find_backend_binary(dir: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let binary_name = "llama-server.exe";
    #[cfg(not(target_os = "windows"))]
    let binary_name = "llama-server";

    // Walk the directory tree to find the binary
    fn walk_for(dir: &Path, name: &str) -> Option<PathBuf> {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.file_name().map(|n| n == name).unwrap_or(false) {
                    return Some(path);
                }
                if path.is_dir() {
                    if let Some(found) = walk_for(&path, name) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    walk_for(dir, binary_name)
        .ok_or_else(|| anyhow!("Could not find {} in extracted archive", binary_name))
}

/// Main entry point for installing a backend.
///
/// Clones `source` from `options` before matching so that `options` fields
/// remain accessible inside each arm.
pub async fn install_backend(options: InstallOptions) -> Result<PathBuf> {
    let source = options.source.clone();
    match source {
        BackendSource::Prebuilt { version } => {
            install_prebuilt(&options, &version).await
        }
        BackendSource::SourceCode { version, git_url } => {
            install_from_source(&options, &version, &git_url).await
        }
    }
}

async fn install_prebuilt(options: &InstallOptions, version: &str) -> Result<PathBuf> {
    tracing::info!("Installing pre-built binary for {:?} version {}", options.backend_type, version);

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let url = get_prebuilt_url(
        &options.backend_type,
        version,
        os,
        arch,
        options.gpu_type.as_ref(),
    )?;

    println!("Downloading from: {}", url);

    let download_dir = tempfile::tempdir()?;
    let archive_name = url.split('/').last().unwrap();
    let archive_path = download_dir.path().join(archive_name);

    download_file(&url, &archive_path).await?;

    println!("Extracting archive...");
    let binary_path = extract_archive(&archive_path, &options.target_dir)?;

    println!("Backend installed at: {:?}", binary_path);
    Ok(binary_path)
}

async fn install_from_source(
    options: &InstallOptions,
    version: &str,
    git_url: &str,
) -> Result<PathBuf> {
    tracing::info!("Building from source: {} version {}", git_url, version);

    // Check prerequisites
    let caps = crate::gpu::detect_system_capabilities();
    if !caps.git_available {
        return Err(anyhow!(
            "Git is required to build from source.\n\
             Install it: https://git-scm.com/downloads\n\
             Linux: sudo apt install git (Debian/Ubuntu) or sudo dnf install git (Fedora)"
        ));
    }
    if !caps.cmake_available {
        return Err(anyhow!(
            "CMake is required to build from source.\n\
             Install it: https://cmake.org/download/\n\
             Linux: sudo apt install cmake (Debian/Ubuntu) or sudo dnf install cmake (Fedora)"
        ));
    }
    if !caps.compiler_available {
        return Err(anyhow!(
            "C++ compiler is required to build from source.\n\
             Linux: sudo apt install build-essential\n\
             Windows: Install Visual Studio Build Tools or MinGW (g++)"
        ));
    }

    let build_dir = tempfile::tempdir()?;
    let source_dir = build_dir.path().join("source");

    // Clone repository
    println!("Cloning repository (shallow)...");
    let clone_result = tokio::process::Command::new("git")
        .args(["clone", "--depth", "1", "--branch", version, git_url, &source_dir.to_string_lossy()])
        .status()
        .await?;

    if !clone_result.success() {
        // If --branch fails (e.g. "main" for ik_llama), try without branch
        println!("Tag/branch '{}' not found, cloning HEAD...", version);
        let status = tokio::process::Command::new("git")
            .args(["clone", "--depth", "1", git_url, &source_dir.to_string_lossy()])
            .status()
            .await?;

        if !status.success() {
            return Err(anyhow!("Failed to clone repository from {}", git_url));
        }
    }

    // Configure CMake
    println!("Configuring CMake...");
    let build_output = build_dir.path().join("build");
    std::fs::create_dir_all(&build_output)?;

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
                cmake_args.push("-DGGML_HIPBLAS=ON".to_string());
            }
            GpuType::CpuOnly => {}
        }
    }

    let status = tokio::process::Command::new("cmake")
        .args(&cmake_args)
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!("CMake configuration failed. Check that all build dependencies are installed."));
    }

    // Build
    let num_jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    println!("Building with {} parallel jobs (this may take several minutes)...", num_jobs);
    let status = tokio::process::Command::new("cmake")
        .args([
            "--build",
            &build_output.to_string_lossy(),
            "--config",
            "Release",
            "-j",
            &num_jobs.to_string(),
        ])
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!("Build failed. Check the output above for errors."));
    }

    // Find and copy the binary
    println!("Installing binary...");
    let binary_src = find_backend_binary(&build_output)?;

    std::fs::create_dir_all(&options.target_dir)?;
    let binary_name = binary_src.file_name().unwrap();
    let binary_dest = options.target_dir.join(binary_name);

    std::fs::copy(&binary_src, &binary_dest)?;

    // Set executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&binary_dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&binary_dest, perms)?;
    }

    println!("Backend built and installed at: {:?}", binary_dest);
    Ok(binary_dest)
}
```

- [ ] **Step 4: Add archive extraction dependencies to kronk-core Cargo.toml**

Add under `[dependencies]`:
```toml
zip = "6.0"
flate2 = "1.0"
tar = "0.4"
```

- [ ] **Step 5: Run tests**

Run: `cargo test --package kronk-core --lib backends::installer`
Expected: PASS

- [ ] **Step 6: Commit installer**

```bash
git add crates/kronk-core/src/backends/installer.rs crates/kronk-core/Cargo.toml
git commit -m "feat(backends): add installer with pre-built download and source build support"
```

---

## Task 4: Update Checker

**Files:**
- Create: `crates/kronk-core/src/backends/updater.rs`

- [ ] **Step 1: Write test for update checking**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_check_llama_cpp_latest() {
        let latest = check_latest_version(&BackendType::LlamaCpp).await;
        assert!(latest.is_ok());
        let version = latest.unwrap();
        assert!(version.starts_with('b'));
    }

    #[tokio::test]
    async fn test_check_ik_llama_latest() {
        // ik_llama only has pre-releases, our code should handle that
        let latest = check_latest_version(&BackendType::IkLlama).await;
        assert!(latest.is_ok());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --package kronk-core --lib backends::updater`
Expected: FAIL with "module not found"

- [ ] **Step 3: Implement update checker**

```rust
// crates/kronk-core/src/backends/updater.rs
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::path::PathBuf;

use super::installer::{install_backend, InstallOptions};
use super::registry::{BackendInfo, BackendRegistry, BackendType};

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[allow(dead_code)]
    prerelease: bool,
}

/// Check the latest release version for a backend.
///
/// For llama.cpp: uses /releases/latest (they have stable releases).
/// For ik_llama: uses /releases (and picks the first one) because
/// they only have pre-releases, and /releases/latest returns 404
/// when no non-prerelease exists.
pub async fn check_latest_version(backend: &BackendType) -> Result<String> {
    let client = Client::builder()
        .user_agent("kronk-backend-manager")
        .build()?;

    match backend {
        BackendType::LlamaCpp => {
            let url = "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";
            let response = client
                .get(url)
                .send()
                .await
                .with_context(|| "Failed to fetch latest llama.cpp release")?;

            if !response.status().is_success() {
                return Err(anyhow!("GitHub API request failed: {}", response.status()));
            }

            let release: GithubRelease = response.json().await?;
            Ok(release.tag_name)
        }
        BackendType::IkLlama => {
            // ik_llama only has pre-releases, so /releases/latest returns 404.
            // Fetch all releases and pick the first (most recent).
            let url = "https://api.github.com/repos/ikawrakow/ik_llama.cpp/releases?per_page=1";
            let response = client
                .get(url)
                .send()
                .await
                .with_context(|| "Failed to fetch ik_llama releases")?;

            if !response.status().is_success() {
                return Err(anyhow!("GitHub API request failed: {}", response.status()));
            }

            let releases: Vec<GithubRelease> = response.json().await?;
            releases
                .first()
                .map(|r| r.tag_name.clone())
                .ok_or_else(|| anyhow!("No releases found for ik_llama"))
        }
        BackendType::Custom => {
            Err(anyhow!("Cannot check updates for custom backends"))
        }
    }
}

pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
}

pub async fn check_updates(backend_info: &BackendInfo) -> Result<UpdateCheck> {
    let latest = check_latest_version(&backend_info.backend_type).await?;

    Ok(UpdateCheck {
        current_version: backend_info.version.clone(),
        latest_version: latest.clone(),
        update_available: latest != backend_info.version,
    })
}

pub async fn update_backend(
    registry: &mut BackendRegistry,
    backend_name: &str,
    options: InstallOptions,
) -> Result<()> {
    // Install the new version
    let new_binary_path = install_backend(options).await?;

    // Fetch the latest version tag (we need it for the registry)
    let backend_info = registry
        .get(backend_name)
        .ok_or_else(|| anyhow!("Backend '{}' not found", backend_name))?;

    let latest = check_latest_version(&backend_info.backend_type).await?;

    // Update registry in one call
    registry.update_version(backend_name, latest, new_binary_path)?;

    println!("Update complete!");
    Ok(())
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --package kronk-core --lib backends::updater`
Expected: PASS (requires network)

- [ ] **Step 5: Commit updater**

```bash
git add crates/kronk-core/src/backends/updater.rs
git commit -m "feat(backends): add update checker with GitHub API integration"
```

---

## Task 5: Update Module Exports

**Files:**
- Modify: `crates/kronk-core/src/backends/mod.rs`

- [ ] **Step 1: Verify mod.rs re-exports are correct**

The `mod.rs` created in Task 2 Step 3 should already have the correct exports. Verify by building:

Run: `cargo build --package kronk-core`
Expected: Compiles successfully

If there are missing re-exports, update `crates/kronk-core/src/backends/mod.rs` to include all public types from each submodule.

- [ ] **Step 2: Commit if any changes needed**

```bash
git add crates/kronk-core/src/backends/mod.rs
git commit -m "fix(backends): update module re-exports"
```

---

## Task 6: CLI Backend Subcommand

**Files:**
- Create: `crates/kronk-cli/src/commands/backend.rs`
- Modify: `crates/kronk-cli/src/commands/mod.rs`
- Modify: `crates/kronk-cli/src/main.rs`

- [ ] **Step 1: Add backend module to commands/mod.rs**

Modify `crates/kronk-cli/src/commands/mod.rs`:
```rust
pub mod backend;
pub mod model;
```

- [ ] **Step 2: Create the CLI command structure**

```rust
// crates/kronk-cli/src/commands/backend.rs
use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use kronk_core::backends::*;
use kronk_core::config::Config;
use kronk_core::gpu;

#[derive(Debug, Args)]
pub struct BackendArgs {
    #[command(subcommand)]
    pub command: BackendSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum BackendSubcommand {
    /// Install a new backend
    Install {
        /// Backend type: llama_cpp or ik_llama
        #[arg(value_name = "TYPE")]
        backend_type: String,

        /// Version to install (e.g., b8407). Defaults to latest.
        #[arg(short, long)]
        version: Option<String>,

        /// Force build from source instead of downloading pre-built binary
        #[arg(long)]
        build: bool,

        /// Custom name for this backend installation
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Update an installed backend to the latest version
    Update {
        /// Name of the backend to update
        name: String,
    },

    /// List installed backends
    #[command(alias = "ls")]
    List,

    /// Remove an installed backend
    #[command(alias = "rm")]
    Remove {
        /// Name of the backend to remove
        name: String,
    },

    /// Check for updates to all installed backends
    CheckUpdates,
}

pub async fn run(cmd: BackendArgs) -> Result<()> {
    match cmd.command {
        BackendSubcommand::Install { backend_type, version, build, name } => {
            cmd_install(&backend_type, version, build, name).await
        }
        BackendSubcommand::Update { name } => cmd_update(&name).await,
        BackendSubcommand::List => cmd_list().await,
        BackendSubcommand::Remove { name } => cmd_remove(&name).await,
        BackendSubcommand::CheckUpdates => cmd_check_updates().await,
    }
}

fn parse_backend_type(s: &str) -> Result<BackendType> {
    match s.to_lowercase().as_str() {
        "llama_cpp" | "llama.cpp" | "llamacpp" => Ok(BackendType::LlamaCpp),
        "ik_llama" | "ik-llama" | "ikllama" | "ik_llama.cpp" => Ok(BackendType::IkLlama),
        _ => Err(anyhow!("Unknown backend type '{}'. Supported: llama_cpp, ik_llama", s)),
    }
}

fn registry_path() -> Result<std::path::PathBuf> {
    let base_dir = Config::base_dir()?;
    Ok(base_dir.join("backend_registry.toml"))
}

fn backends_dir() -> Result<std::path::PathBuf> {
    let base_dir = Config::base_dir()?;
    Ok(base_dir.join("backends"))
}

fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn cmd_install(
    backend_type_str: &str,
    version: Option<String>,
    force_build: bool,
    name: Option<String>,
) -> Result<()> {
    let backend_type = parse_backend_type(backend_type_str)?;

    // Detect system capabilities
    println!("Detecting system capabilities...");
    let caps = gpu::detect_system_capabilities();
    println!("  OS:       {} {}", caps.os, caps.arch);
    println!("  Git:      {}", if caps.git_available { "found" } else { "not found" });
    println!("  CMake:    {}", if caps.cmake_available { "found" } else { "not found" });
    println!("  Compiler: {}", if caps.compiler_available { "found" } else { "not found" });

    if let Some(ref gpu) = caps.gpu {
        println!("  GPU:      {} ({:?})", gpu.device_name, gpu.gpu_type);
        println!("  VRAM:     {} MiB", gpu.vram_mb);
    } else {
        println!("  GPU:      none detected (CPU-only)");
    }

    // Fetch latest version if not specified
    let version = match version {
        Some(v) => v,
        None => {
            println!("\nFetching latest version...");
            check_latest_version(&backend_type).await?
        }
    };
    println!("Version: {}", version);

    // Determine installation method.
    // ik_llama has no pre-built binaries, so source is the only option.
    let use_source = match backend_type {
        BackendType::IkLlama => {
            if !force_build {
                println!("\nik_llama does not provide pre-built binaries. Building from source.");
            }
            true
        }
        _ if force_build => true,
        _ => {
            let choice = inquire::Select::new(
                "Installation method:",
                vec!["Download pre-built binary (faster)", "Build from source (hardware-optimized)"],
            )
            .prompt()?;
            choice.starts_with("Build")
        }
    };

    // Confirm GPU settings
    let gpu_type = if let Some(ref gpu_cap) = caps.gpu {
        println!("\nDetected GPU: {} ({:?})", gpu_cap.device_name, gpu_cap.gpu_type);
        let confirm = inquire::Confirm::new("Use this GPU for acceleration?")
            .with_default(true)
            .prompt()?;

        if confirm {
            Some(gpu_cap.gpu_type.clone())
        } else {
            None
        }
    } else {
        None
    };

    // Determine install directory
    let backend_name = name.unwrap_or_else(|| {
        let type_str = match backend_type {
            BackendType::LlamaCpp => "llama_cpp",
            BackendType::IkLlama => "ik_llama",
            BackendType::Custom => "custom",
        };
        format!("{}_{}", type_str, version)
    });

    let target_dir = backends_dir()?.join(&backend_name);

    // Build install options
    let git_url = match backend_type {
        BackendType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
        BackendType::IkLlama => "https://github.com/ikawrakow/ik_llama.cpp.git",
        BackendType::Custom => unreachable!(),
    };

    let source = if use_source {
        BackendSource::SourceCode {
            version: version.clone(),
            git_url: git_url.to_string(),
        }
    } else {
        BackendSource::Prebuilt {
            version: version.clone(),
        }
    };

    let options = InstallOptions {
        backend_type: backend_type.clone(),
        source,
        target_dir,
        gpu_type: gpu_type.clone(),
    };

    // Install
    println!("\nStarting installation...");
    let binary_path = install_backend(options).await?;

    // Register
    let mut registry = BackendRegistry::load(&registry_path()?)?;
    registry.add(BackendInfo {
        name: backend_name.clone(),
        backend_type,
        version,
        path: binary_path.clone(),
        installed_at: current_unix_timestamp(),
        gpu_type,
    })?;

    println!("\nInstallation complete!");
    println!("  Name:   {}", backend_name);
    println!("  Binary: {}", binary_path.display());
    println!("\nTo use this backend:");
    println!("  kronk server add my-server {} --host 0.0.0.0 -m model.gguf -ngl 999", binary_path.display());

    Ok(())
}

async fn cmd_update(name: &str) -> Result<()> {
    let mut registry = BackendRegistry::load(&registry_path()?)?;

    let backend_info = registry
        .get(name)
        .ok_or_else(|| anyhow!("Backend '{}' not found. Run `kronk backend list` to see installed backends.", name))?
        .clone();

    println!("Checking for updates to '{}'...", name);
    let update_check = check_updates(&backend_info).await?;

    if !update_check.update_available {
        println!("'{}' is already up to date ({})", name, backend_info.version);
        return Ok(());
    }

    println!("Update available:");
    println!("  Current: {}", update_check.current_version);
    println!("  Latest:  {}", update_check.latest_version);

    let confirm = inquire::Confirm::new("Proceed with update?")
        .with_default(true)
        .prompt()?;

    if !confirm {
        println!("Update cancelled.");
        return Ok(());
    }

    let target_dir = backend_info.path.parent().unwrap().to_path_buf();

    let source = BackendSource::Prebuilt {
        version: update_check.latest_version.clone(),
    };

    let options = InstallOptions {
        backend_type: backend_info.backend_type.clone(),
        source,
        target_dir,
        gpu_type: backend_info.gpu_type.clone(),
    };

    update_backend(&mut registry, name, options).await?;

    Ok(())
}

async fn cmd_list() -> Result<()> {
    let registry = BackendRegistry::load(&registry_path()?)?;
    let backends = registry.list();

    if backends.is_empty() {
        println!("No backends installed.");
        println!("\nTo install one:");
        println!("  kronk backend install llama_cpp");
        println!("  kronk backend install ik_llama");
        return Ok(());
    }

    println!("Installed backends:\n");
    for backend in backends {
        println!("  {} ({:?})", backend.name, backend.backend_type);
        println!("    Version: {}", backend.version);
        println!("    Path:    {}", backend.path.display());
        if let Some(ref gpu) = backend.gpu_type {
            println!("    GPU:     {:?}", gpu);
        }
        println!();
    }

    Ok(())
}

async fn cmd_remove(name: &str) -> Result<()> {
    let mut registry = BackendRegistry::load(&registry_path()?)?;

    let backend = registry
        .get(name)
        .ok_or_else(|| anyhow!("Backend '{}' not found", name))?
        .clone();

    println!("Removing backend '{}'", name);
    println!("  Path: {}", backend.path.display());

    let confirm = inquire::Confirm::new("Are you sure?")
        .with_default(false)
        .prompt()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    // Remove from registry first
    registry.remove(name)?;

    // Optionally remove files
    if backend.path.exists() {
        let remove_files = inquire::Confirm::new("Also delete the backend files from disk?")
            .with_default(true)
            .prompt()?;

        if remove_files {
            if let Some(parent) = backend.path.parent() {
                // Safety: only remove if it's under our managed backends dir
                let managed = backends_dir()?;
                if parent.starts_with(&managed) {
                    std::fs::remove_dir_all(parent)?;
                    println!("Files removed.");
                } else {
                    println!("Skipping file removal: path is outside managed directory.");
                }
            }
        }
    }

    println!("Backend '{}' removed.", name);
    Ok(())
}

async fn cmd_check_updates() -> Result<()> {
    let registry = BackendRegistry::load(&registry_path()?)?;
    let backends = registry.list();

    if backends.is_empty() {
        println!("No backends installed.");
        return Ok(());
    }

    println!("Checking for updates...\n");

    for backend in backends {
        print!("  {} ({}): ", backend.name, backend.version);

        match check_updates(backend).await {
            Ok(check) => {
                if check.update_available {
                    println!("UPDATE AVAILABLE -> {}", check.latest_version);
                } else {
                    println!("up to date");
                }
            }
            Err(e) => {
                println!("error: {}", e);
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Add Backend variant to Commands enum in main.rs**

In `crates/kronk-cli/src/main.rs`, add to the `Commands` enum (around line 86-100):
```rust
    /// Manage LLM backends (install, update, remove)
    Backend(commands::backend::BackendArgs),
```

And add the match arm in the main dispatch (find the existing `Commands::Model` match arm and add after it):
```rust
    Commands::Backend(args) => commands::backend::run(args).await?,
```

- [ ] **Step 4: Test CLI compiles**

Run: `cargo build --package kronk-cli`
Expected: Compiles successfully

- [ ] **Step 5: Commit CLI integration**

```bash
git add crates/kronk-cli/src/commands/backend.rs crates/kronk-cli/src/commands/mod.rs crates/kronk-cli/src/main.rs
git commit -m "feat(cli): add backend subcommand for install/update/list/remove"
```

---

## Task 7: Integration Tests

**Files:**
- New inline tests in the existing modules (per-crate, not workspace root)

The unit tests in Tasks 1-4 already cover the core logic. This task adds a broader integration test to `kronk-core`.

- [ ] **Step 1: Add integration-style test to registry**

Add to `crates/kronk-core/src/backends/registry.rs` tests:

```rust
#[test]
fn test_registry_update_version() {
    let tmp = TempDir::new().unwrap();
    let registry_path = tmp.path().join("registry.toml");
    let mut registry = BackendRegistry::load(&registry_path).unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    registry.add(BackendInfo {
        name: "test".to_string(),
        backend_type: BackendType::LlamaCpp,
        version: "b8400".to_string(),
        path: "/old/path".into(),
        installed_at: now,
        gpu_type: None,
    }).unwrap();

    registry.update_version("test", "b8407".to_string(), "/new/path".into()).unwrap();

    let updated = registry.get("test").unwrap();
    assert_eq!(updated.version, "b8407");
    assert_eq!(updated.path, std::path::PathBuf::from("/new/path"));
}
```

- [ ] **Step 2: Run all tests**

Run: `cargo test --package kronk-core`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/kronk-core/src/backends/registry.rs
git commit -m "test: add registry update version test"
```

---

## Task 8: Documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add backend management section to README**

Add after the "Quick Start" section and before "CLI":

```markdown
## Backend Management

Kronk can install and manage LLM backends for you.

### Install a backend

```bash
# Install latest llama.cpp (interactive: choose pre-built or source)
kronk backend install llama_cpp

# Install specific version
kronk backend install llama_cpp --version b8407

# Force build from source
kronk backend install llama_cpp --build

# Install ik_llama (builds from source -- no pre-built binaries available)
kronk backend install ik_llama
```

During installation, Kronk will:
1. Detect your GPU (CUDA, Vulkan, etc.)
2. Ask your preferred installation method (pre-built or source)
3. Confirm GPU acceleration settings
4. Download/build and install the backend

### Manage backends

```bash
kronk backend list              # List installed backends
kronk backend check-updates     # Check for new versions
kronk backend update <name>     # Update to latest version
kronk backend remove <name>     # Remove an installed backend
```

Backends are stored in `~/.config/kronk/backends/` (Linux) or `%APPDATA%\kronk\backends\` (Windows). You can also point servers at external backend installations via the config file.

- [ ] **Step 2: Add backend commands to the CLI reference table**

Add to the CLI table in README.md:
```bash
kronk backend install <type>                       Install a backend (llama_cpp, ik_llama)
kronk backend list                                 List installed backends
kronk backend update <name>                        Update a backend to latest
kronk backend remove <name>                        Remove an installed backend
kronk backend check-updates                        Check for available updates
```

- [ ] **Step 3: Commit documentation**

```bash
git add README.md
git commit -m "docs: add backend management documentation"
```

---

## Task 9: Final Verification

**Files:**
- All files

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Run format check**

Run: `cargo fmt --all -- --check`
Expected: All files formatted

- [ ] **Step 4: Build release binary**

Run: `cargo build --release --workspace`
Expected: Build succeeds

- [ ] **Step 5: Smoke test - help text**

Run: `./target/release/kronk backend --help`
Expected: Shows install/update/list/remove/check-updates subcommands

- [ ] **Step 6: Smoke test - list backends**

Run: `./target/release/kronk backend list`
Expected: Shows "No backends installed" with install hint

---

## Revision Notes (fixes from review)

This plan addresses all 16 issues identified in review:

1. **Config::base_dir() is static** -- CLI now calls `Config::base_dir()?` (static), not `config.base_dir()` (instance). Extracted into `registry_path()` and `backends_dir()` helpers.
2. **Variable shadowing in get_prebuilt_url** -- Renamed parameter to `tag` to avoid shadowing with CUDA `version`. GPU type accessed via `ref version` only where needed.
3. **tar.gz extension matching** -- Now matches on full filename (`ends_with(".tar.gz")`) instead of single extension.
4. **Existing gpu.rs duplicated** -- No separate `detector.rs`. All GPU detection extends the existing `gpu.rs` module, reusing its nvidia-smi patterns.
5. **SystemTime TOML serialization** -- Changed `installed_at` from `SystemTime` to `i64` (unix epoch seconds).
6. **install_backend ownership issue** -- Clones `options.source` before matching so `options` remains borrowable.
7. **inquire dependency** -- Already in workspace deps and kronk-cli's Cargo.toml. No changes needed (noted).
8. **ik_llama /releases/latest returns 404** -- Uses `/releases?per_page=1` endpoint instead, which returns pre-releases.
9. **ik_llama pre-built fails silently** -- Install flow now skips the "pre-built or source" prompt for ik_llama, always uses source.
10. **Integration test location** -- Tests are inline in each module (standard Rust pattern), not in nonexistent workspace `tests/` dir.
11. **commands module import** -- Adds `pub mod backend;` to existing `commands/mod.rs`, not a new `mod commands` block.
12. **config.rs modification listed but never described** -- Removed from "Files to Modify" since no config.rs changes are actually needed.
13. **find_backend_binary too shallow** -- Now does recursive directory walk to find the binary.
14. **CUDA detection requires nvcc** -- Now parses CUDA version from nvidia-smi header output (doesn't require CUDA toolkit).
15. **num_cpus unnecessary** -- Replaced with `std::thread::available_parallelism()`.
16. **gpu_type ownership in CLI** -- Uses `gpu_cap.gpu_type.clone()` to avoid moving out of borrowed caps.

Additional improvements:
- Safety check in `cmd_remove`: only deletes files under managed backends directory
- Better error messages with installation hints for missing cmake/compiler
- Git clone fallback when branch/tag not found
- `alias = "ls"` and `alias = "rm"` on list/remove subcommands for consistency with other kronk commands
- Used `tracing::info!` instead of `println!` for internal progress (matching existing patterns)
- User-Agent header on all HTTP requests (GitHub API requires it)
- Compiler detection checks for `g++`/`c++` (C++ compiler, not just C)

## Revision 2: Portability (runs on many machines)

17. **Pure-Rust archive extraction** -- Replaced shell-out to `tar` with `flate2` + `tar` crates. The external `tar` command may not be in PATH on older Windows systems. Now uses pure Rust for both .tar.gz (flate2+tar) and .zip (zip crate). No external archive tools required.
18. **Git prerequisite check** -- Added `git_available: bool` to `SystemCapabilities`. Source builds now fail early with a clear install URL if git is missing, rather than a cryptic "command not found" from the shell.
19. **Windows compiler detection covers MinGW** -- On Windows, compiler detection now tries both `cl.exe` (MSVC) and `g++` (MinGW/MSYS2). Many Windows users use MinGW rather than the full Visual Studio Build Tools.
