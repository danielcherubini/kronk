# Replace GPU Auto-Detection with Interactive Prompts

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove ~300 lines of fragile, untestable GPU parsing code (`rocminfo`, `vulkaninfo`, `system_profiler`) and replace it with a simple interactive prompt that asks the user what GPU they have.

**Architecture:** Move GPU selection into the CLI layer (`commands/backend.rs`) using `inquire` prompts. Keep the existing `GpuType` enum and `query_vram()` / `suggest_context_sizes()` which are used elsewhere and work fine (nvidia-smi is reliable). Remove `detect_gpu()`, `detect_rocm()`, `detect_vulkan_gpu()`, `detect_metal_gpu()`, `detect_rocm_from_lines()`, `detect_rocm_impl()`, and all their tests. Split `detect_system_capabilities()` into a simpler `detect_build_prerequisites()` that only checks for git/cmake/compiler (no GPU). The `SystemCapabilities` struct loses its `gpu` field.

**Tech Stack:** Rust, `inquire` 0.7 (already a dependency of `kronk-cli`)

---

## Task 1: Remove GPU Detection Functions from `gpu.rs`

**Files:**
- Modify: `crates/kronk-core/src/gpu.rs`

- [ ] **Step 1: Write a failing test for `detect_build_prerequisites`**

Add a test that calls the new function and verifies it returns the struct without a `gpu` field:

```rust
#[test]
fn test_detect_build_prerequisites() {
    let caps = detect_build_prerequisites();
    assert!(!caps.os.is_empty());
    assert!(!caps.arch.is_empty());
    // No gpu field — that's the point
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kronk-core test_detect_build_prerequisites`
Expected: FAIL — `detect_build_prerequisites` doesn't exist yet.

- [ ] **Step 3: Rename `SystemCapabilities` and remove `gpu` field**

In `crates/kronk-core/src/gpu.rs`:

1. Rename `SystemCapabilities` to `BuildPrerequisites`:
```rust
#[derive(Debug, Clone)]
pub struct BuildPrerequisites {
    pub os: String,
    pub arch: String,
    pub cmake_available: bool,
    pub compiler_available: bool,
    pub git_available: bool,
}
```

2. Rename `detect_system_capabilities()` to `detect_build_prerequisites()` and remove the `detect_gpu()` call:
```rust
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
```

3. Delete all GPU detection functions:
   - `detect_gpu()`
   - `detect_cuda_gpu()`
   - `detect_cuda_version_from_smi()`
   - `detect_rocm()`
   - `detect_rocm_from_lines()`
   - `detect_rocm_impl()`
   - `detect_vulkan_gpu()`
   - `parse_vulkan_device_local_heap()`
   - `detect_metal_gpu()` (both `cfg` variants)
   - `GpuCapability` struct

4. Delete all tests for removed functions:
   - `test_detect_system_capabilities` (replace with `test_detect_build_prerequisites`)
   - `test_detect_rocm`
   - `test_gpu_type_display`

Keep intact:
- `GpuType` enum (used by `installer.rs`, `registry.rs` for serialization)
- `VramInfo` struct + `query_vram()` (used by `main.rs` status, `model.rs` context sizing)
- `suggest_context_sizes()` + `ContextSuggestion` (used by `model.rs`)
- All VRAM/context tests (`test_vram_info_available`, `test_suggest_context_sizes_*`)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kronk-core test_detect_build_prerequisites`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/kronk-core/src/gpu.rs
git commit -m "refactor: replace GPU auto-detection with BuildPrerequisites

Remove ~250 lines of fragile rocminfo/vulkaninfo/nvidia-smi/system_profiler
GPU parsing. GPU selection will be handled by interactive prompts in the CLI.
Keep GpuType enum, query_vram(), and suggest_context_sizes() which are used
elsewhere and work reliably."
```

---

### Task 2: Update `installer.rs` to Use `BuildPrerequisites`

**Files:**
- Modify: `crates/kronk-core/src/backends/installer.rs`

- [ ] **Step 1: Update the import and `install_from_source` call**

In `crates/kronk-core/src/backends/installer.rs`:

1. The import of `crate::gpu::GpuType` stays (used by `InstallOptions` and cmake flags).

2. Change `install_from_source` to call `detect_build_prerequisites()` instead of `detect_system_capabilities()`:

```rust
// Line 371: change from
let caps = crate::gpu::detect_system_capabilities();
// to
let caps = crate::gpu::detect_build_prerequisites();
```

No other changes needed — the function only uses `caps.git_available`, `caps.cmake_available`, `caps.compiler_available`. It never touches `caps.gpu`.

- [ ] **Step 2: Verify compilation**

Run: `cargo build -p kronk-core`
Expected: Compiles with 0 errors.

- [ ] **Step 3: Run existing tests**

Run: `cargo test -p kronk-core`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/kronk-core/src/backends/installer.rs
git commit -m "fix: use detect_build_prerequisites in installer"
```

---

### Task 3: Update CLI `backend.rs` — Interactive GPU Prompt

**Files:**
- Modify: `crates/kronk-cli/src/commands/backend.rs`

- [ ] **Step 1: Replace auto-detection with prompt in `cmd_install`**

In `crates/kronk-cli/src/commands/backend.rs`, make these changes to `cmd_install()`:

**Replace lines 102-115** (the `detect_system_capabilities` call and its display) with:
```rust
    // Check build prerequisites
    println!("Checking system...");
    let caps = gpu::detect_build_prerequisites();
    println!("  OS:       {} {}", caps.os, caps.arch);
    println!("  Git:      {}", if caps.git_available { "found" } else { "not found" });
    println!("  CMake:    {}", if caps.cmake_available { "found" } else { "not found" });
    println!("  Compiler: {}", if caps.compiler_available { "found" } else { "not found" });
    println!();
```

**Keep lines 117-145 unchanged** (version fetching + install method selection).

**Replace lines 147-161** (the GPU auto-detect confirm block) with:
```rust
    // Ask user about GPU acceleration
    let gpu_type = {
        let gpu_choice = inquire::Select::new(
            "What GPU acceleration do you want?",
            vec![
                "NVIDIA (CUDA)",
                "AMD (ROCm)",
                "Intel / AMD (Vulkan)",
                "Apple Silicon (Metal)",
                "CPU only",
            ],
        )
        .prompt()?;

        match gpu_choice {
            "NVIDIA (CUDA)" => Some(gpu::GpuType::Cuda {
                version: "auto".to_string(),
            }),
            "AMD (ROCm)" => Some(gpu::GpuType::RocM {
                version: "auto".to_string(),
            }),
            "Intel / AMD (Vulkan)" => Some(gpu::GpuType::Vulkan),
            "Apple Silicon (Metal)" => Some(gpu::GpuType::Metal),
            _ => None,
        }
    };
```

Note: `main.rs` does NOT reference `SystemCapabilities` or `detect_system_capabilities` — it only uses `query_vram()` at line 898, which is being kept, so `main.rs` needs no changes.

Note: For CUDA and ROCm, we use `version: "auto"` since the exact version is only needed for selecting pre-built binary URLs, and llama.cpp release naming uses fixed versions (e.g. `rocm-7.2`), not the user's actual ROCm version.

- [ ] **Step 2: Update the import**

Change:
```rust
use kronk_core::gpu;
```
This stays the same — we still use `gpu::detect_build_prerequisites()` and `gpu::GpuType`.

Remove any reference to `GpuCapability` or `SystemCapabilities` if present. (Check: the file uses `caps.gpu` which is a `GpuCapability` — this entire access pattern gets replaced by the prompt.)

- [ ] **Step 3: Verify compilation**

Run: `cargo build -p kronk`
Expected: Compiles with 0 errors.

- [ ] **Step 4: Commit**

```bash
git add crates/kronk-cli/src/commands/backend.rs
git commit -m "feat: replace GPU auto-detection with interactive prompt

Instead of trying to parse rocminfo/vulkaninfo output (fragile, untestable),
just ask the user what GPU they have. They know."
```

---

### Task 4: Clean Up Remaining References

**Files:**
- Modify: `crates/kronk-cli/src/main.rs` (if it references `SystemCapabilities`)
- Modify: `crates/kronk-core/src/backends/registry.rs` (check `GpuType` usage is fine)

- [ ] **Step 1: Search for stale references**

Run: `grep -rn "SystemCapabilities\|detect_system_capabilities\|GpuCapability\|detect_gpu\|detect_rocm\|detect_vulkan\|detect_metal\|detect_cuda" crates/`

Fix any remaining references. The expected ones that should stay:
- `GpuType` in `registry.rs`, `installer.rs`, `backend.rs` — these are fine
- `query_vram` in `main.rs`, `model.rs` — fine
- `suggest_context_sizes` in `model.rs` — fine

- [ ] **Step 2: Fix any compilation errors**

Run: `cargo build`
Expected: Compiles with 0 errors and no warnings related to removed items.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: All tests pass (should be 63+ tests, minus deleted ROCm/GPU tests, plus new `test_detect_build_prerequisites`).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: clean up stale GPU detection references"
```

---

### Task 5: Fix Remaining Code Review Items

These are the other issues from the review that don't involve GPU detection.

**Files:**
- Modify: `crates/kronk-core/src/backends/registry.rs`
- Modify: `crates/kronk-core/src/backends/installer.rs`

- [ ] **Step 5a: Fix `BackendRegistry::load_with_base_dir` for first-use (non-existent file)**

In `crates/kronk-core/src/backends/registry.rs`, `load_with_base_dir()` calls `canonicalize(path)` at line 83 which fails if the file doesn't exist yet (first use). The fix must preserve the security validation (base-dir containment check) while handling non-existent files.

Replace lines 82-84 in `load_with_base_dir`:
```rust
        let canonical_path = std::fs::canonicalize(path)
            .with_context(|| format!("Failed to canonicalize registry path {:?}", path))?;
```

With:
```rust
        // For non-existent files (first use), canonicalize the parent directory
        // and append the filename. This preserves the symlink-attack check.
        let canonical_path = if path.exists() {
            std::fs::canonicalize(path)
                .with_context(|| format!("Failed to canonicalize registry path {:?}", path))?
        } else {
            let parent = path
                .parent()
                .ok_or_else(|| anyhow!("Registry path {:?} has no parent directory", path))?;
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {:?}", parent))?;
            let canonical_parent = std::fs::canonicalize(parent)
                .with_context(|| format!("Failed to canonicalize parent {:?}", parent))?;
            let file_name = path
                .file_name()
                .ok_or_else(|| anyhow!("Registry path {:?} has no filename", path))?;
            canonical_parent.join(file_name)
        };
```

The rest of `load_with_base_dir` (the `starts_with` check and data loading) stays the same — it already handles non-existent files at line 103-108.

- [ ] **Step 5b: Write test for first-use registry loading**

Use `load_with_base_dir` with an explicit base dir (since `load()` calls `Config::base_dir()` which may not match our temp dir):

```rust
#[test]
fn test_load_nonexistent_creates_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nonexistent_registry.toml");
    let registry = BackendRegistry::load_with_base_dir(&path, Some(dir.path())).unwrap();
    assert!(registry.list().is_empty());
}
```

- [ ] **Step 5c: Run tests**

Run: `cargo test -p kronk-core`
Expected: All pass.

- [ ] **Step 5d: Commit**

```bash
git add crates/kronk-core/src/backends/registry.rs
git commit -m "fix: handle first-use registry load when file doesn't exist

canonicalize() fails on non-existent paths. For first use, canonicalize the
parent directory instead and append the filename, preserving the symlink
attack prevention check."
```

---

### Task 6: Final Verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass with 0 failures.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Verify no dead code warnings**

Run: `cargo build 2>&1 | grep warning`
Expected: No warnings about unused functions (the old `detect_rocm_from_lines` and `detect_rocm_impl` warnings should be gone).
