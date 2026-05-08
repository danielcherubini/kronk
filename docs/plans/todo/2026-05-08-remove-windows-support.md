# Remove Windows Support Plan

**Goal:** Make tama Linux-only by removing all Windows-specific code, CI, build targets, dependencies, and documentation.

**Architecture:** Strip `#[cfg(windows)]` branches throughout three crates (tama-core, tama-cli, tama-web), delete the Windows platform module and installer, remove Windows CI/release jobs, and update docs. The `platform/` module structure is preserved for future Windows/BSD support. macOS match arms in `urls.rs` and `#[cfg(unix)]` code paths are intentionally preserved — they are unreachable at compile time because `platform/mod.rs` gates non-Linux builds with `compile_error!`.

**Tech Stack:** Rust, Cargo, GitHub Actions, Inno Setup (removed)

---

## Task 1: Delete Windows files and update platform module

**Context:**
Remove the Windows platform directory, job object module, and installer directory. Update `platform/mod.rs` to only export Linux, with a `compile_error!` for non-Linux builds (updated wording for future BSD support).

**Files:**
- Delete: `crates/tama-core/src/platform/windows/` (entire directory — firewall.rs, install.rs, mod.rs, permissions.rs, service.rs)
- Delete: `crates/tama-core/src/platform/job_object.rs`
- Delete: `installer/` (entire directory — tama.iss)
- Modify: `crates/tama-core/src/platform/mod.rs`

**What to implement:**
Replace `platform/mod.rs` with:
```rust
pub mod linux;
// Windows and BSD support planned for future releases

#[cfg(not(target_os = "linux"))]
compile_error!("Tama currently only supports Linux. BSD support is planned.");
```

**Steps:**
- [ ] Delete `crates/tama-core/src/platform/windows/` directory
- [ ] Delete `crates/tama-core/src/platform/job_object.rs`
- [ ] Delete `installer/` directory
- [ ] Edit `crates/tama-core/src/platform/mod.rs` — replace entire file with the code above
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "chore: remove Windows platform module and installer"

**Acceptance criteria:**
- [ ] `platform/windows/` directory does not exist
- [ ] `job_object.rs` does not exist
- [ ] `installer/` directory does not exist
- [ ] `platform/mod.rs` only exports `linux` module
- [ ] `cargo check --workspace` passes

---

## Task 2: Strip Windows code from tama-core

**Context:**
Remove all `#[cfg(windows)]` and `#[cfg(target_os = "windows")]` branches from tama-core. For each file, make the Linux/unix branch unconditional. Also strip Windows runtime `cfg!()` checks, Windows match arms in URL construction, and Windows-specific tests.

**Files:**
- Modify: `crates/tama-core/src/backends/installer/source/detect.rs`
- Modify: `crates/tama-core/src/backends/installer/source/build.rs`
- Modify: `crates/tama-core/src/backends/installer/source/install.rs`
- Modify: `crates/tama-core/src/backends/installer/extract.rs`
- Modify: `crates/tama-core/src/backends/installer/urls.rs`
- Modify: `crates/tama-core/src/backends/mod.rs`
- Modify: `crates/tama-core/src/config/rename_legacy.rs`
- Modify: `crates/tama-core/src/config/loader.rs`
- Modify: `crates/tama-core/src/proxy/server/mod.rs`
- Modify: `crates/tama-core/src/proxy/process.rs`
- Modify: `crates/tama-core/src/bench/llama_bench/discovery.rs`
- Modify: `crates/tama-core/src/bench/llama_cli_spec/discovery.rs`
- Modify: `crates/tama-core/src/gpu/detect.rs`
- Modify: `crates/tama-core/src/self_update.rs`

**What to implement:**

For each file, remove Windows branches and make the Linux/unix path unconditional:

1. **`backends/installer/source/detect.rs`** — Remove `#[cfg(target_os = "windows")]` detect functions. Make the `#[cfg(not(target_os = "windows"))]` functions unconditional (remove the attribute).

2. **`backends/installer/source/build.rs`** — Remove the `cfg!(target_os = "windows")` runtime block (lines ~83-97, Ninja/clang-cl cmake args). Remove the `#[cfg(target_os = "windows")]` test `test_ik_llama_windows_uses_ninja_clang_cl_avx2`. Remove `#[cfg(not(target_os = "windows"))]` gates from the `use crate::backends::installer::source::detect;` import and the three `test_hip_env_from_hipconfig_output_*` functions (make them unconditional).

3. **`backends/installer/source/install.rs`** — Remove `#[cfg(target_os = "windows")]` import blocks (`find_llvm_bin`, `find_vcvarsall`). Remove Windows install functions. Make `anyhow::Context` import unconditional. Make `detect_hip_env` import unconditional.

4. **`backends/installer/extract.rs`** — In `find_backend_binary()`, hardcode `"llama-server"` (remove the `.exe` conditional). Keep the `.zip` extraction path — it's unconditional code that could be useful for future backends.

5. **`backends/installer/urls.rs`** — Remove all `("windows", ...)` match arms from `get_prebuilt_url()`. Remove Windows-specific tests: `test_llama_cpp_download_url_windows_cuda`, `test_llama_cpp_download_url_windows_vulkan`, `test_llama_cpp_download_url_windows_cuda13`, `test_supported_cuda_versions_all_map_to_urls` (tests ALL SUPPORTED_CUDA_VERSIONS against Windows URLs). **Note:** macOS match arms are intentionally preserved for future re-support — they are unreachable because `platform/mod.rs` gates non-Linux with `compile_error!`.

6. **`backends/mod.rs`** — Remove the `#[cfg(windows)]` block and make the `#[cfg(not(windows))]` block unconditional (remove the `#[cfg(not(windows))]` attribute).

7. **`config/rename_legacy.rs`** — Remove the `#[cfg(target_os = "windows")]` block and make the `#[cfg(not(target_os = "windows"))]` block unconditional (remove the attribute, keep the code). Update the module doc comment — remove `%APPDATA%\kronk (Windows)` reference.

8. **`config/loader.rs`** — Remove Windows branch. Update doc comments — remove Windows references (`%APPDATA%\tama`, "Windows service which runs as SYSTEM", "Used by tests and Windows service").

9. **`proxy/server/mod.rs`** — Remove the `#[cfg(windows)]` taskkill branch (line ~273). Make the `#[cfg(unix)]` kill command unconditional (remove the `#[cfg(unix)]` gate).

10. **`proxy/process.rs`** — Remove all `#[cfg(windows)]` branches (is_process_alive, kill_process, force_kill_process, configure_process_group, kill_process_group, force_kill_process_group). Make `#[cfg(unix)]` branches unconditional (remove the `#[cfg(unix)]` gate). Update doc comments — remove "On Windows" references.

11. **`bench/llama_bench/discovery.rs`** — Hardcode `"llama-bench"` (remove the `.exe` conditional at line ~24).

12. **`bench/llama_cli_spec/discovery.rs`** — Hardcode `"llama-server"` in both the main `server_name` variable (line ~32) and the `server_name()` test helper function (line ~90). Remove both `cfg!(target_os = "windows")` checks.

13. **`gpu/detect.rs`** — Remove the `#[cfg(target_os = "windows")]` GPU detection branch. Make the `#[cfg(not(target_os = "windows"))]` branch unconditional.

14. **`self_update.rs`** — Hardcode `"tama"` in `target_binary_name()` (remove the `cfg!(target_os = "windows")` check). Hardcode `"tama"` in `perform_update_sync()`'s `bin_name` (remove the `cfg!(target_os = "windows")` check). In `is_running_as_service()`, remove the `#[cfg(target_os = "windows")]` block and make the `#[cfg(target_os = "linux")]` block unconditional. In `restart_as_service()`, remove the `#[cfg(target_os = "windows")]` block (which calls the deleted `platform::windows::restart_service`) and remove the `#[cfg(not(any(target_os = "linux", target_os = "windows")))]` fallback. Remove the `#[cfg(target_os = "windows")]` test `test_target_binary_name_windows`. Update the `test_detect_archive_kind_zip` test — change the filename from `"tama-x86_64-pc-windows-msvc.zip"` to `"tama-x86_64-unknown-linux-gnu.zip"` (the test is about zip detection, not Windows; the filename doesn't matter as long as it ends in `.zip`).

**Steps:**
- [ ] Edit each file listed above — remove Windows branches, make Linux branches unconditional
- [ ] Update doc comments to remove Windows references in `proxy/process.rs`, `config/loader.rs`, `config/rename_legacy.rs`
- [ ] Run `cargo check --package tama-core`
  - Did it succeed? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "chore: strip Windows code from tama-core"

**Acceptance criteria:**
- [ ] No `#[cfg(windows)]` or `#[cfg(target_os = "windows")]` in any tama-core `.rs` file
- [ ] No `cfg!(target_os = "windows")` runtime checks in tama-core
- [ ] No `("windows", ...)` match arms in urls.rs
- [ ] `cargo test --package tama-core` passes
- [ ] `cargo check --package tama-core` passes

---

## Task 3: Strip Windows code from tama-cli

**Context:**
Remove all Windows service dispatch, service management, and handler code from tama-cli. The `service.rs` file becomes entirely dead code after stripping Windows — delete it. Update `lib.rs` to remove the Windows service dispatch check.

**Files:**
- Delete: `crates/tama-cli/src/service.rs` (entirely dead code after Windows removal)
- Modify: `crates/tama-cli/src/lib.rs`
- Modify: `crates/tama-cli/src/handlers/service_cmd.rs`
- Modify: `crates/tama-cli/src/handlers/server/ls.rs`
- Modify: `crates/tama-cli/src/handlers/server/rm.rs`

**What to implement:**

1. **`service.rs`** — Delete the entire file. All its content is `#[cfg(target_os = "windows")]` or the `#[cfg(not(target_os = "windows"))]` stubs that just bail.

2. **`lib.rs`** — Remove `pub mod service;` declaration. Remove `#[cfg(target_os = "windows")] use service::service_dispatch;`. Remove the `#[cfg(target_os = "windows")]` block that checks for `"service-run"` and calls `service_dispatch()`.

3. **`handlers/service_cmd.rs`** — Remove all `#[cfg(target_os = "windows")]` blocks. Remove all `#[cfg(not(any(target_os = "windows", target_os = "linux")))]` fallback blocks (Linux is the only target). Make `#[cfg(target_os = "linux")]` blocks unconditional (remove the attribute).

4. **`handlers/server/ls.rs`** — Remove the `#[cfg(target_os = "windows")]` `query_service` branch. Remove the `#[cfg(not(any(target_os = "windows", target_os = "linux")))]` fallback.

5. **`handlers/server/rm.rs`** — Remove the `#[cfg(target_os = "windows")]` `query_service` branch. Remove the `#[cfg(not(any(target_os = "windows", target_os = "linux")))]` fallback.

**Steps:**
- [ ] Delete `crates/tama-cli/src/service.rs`
- [ ] Edit `lib.rs` — remove `pub mod service`, Windows import, and Windows service dispatch block
- [ ] Edit `handlers/service_cmd.rs` — remove Windows and unsupported fallback blocks, make Linux blocks unconditional
- [ ] Edit `handlers/server/ls.rs` — remove Windows and fallback blocks
- [ ] Edit `handlers/server/rm.rs` — remove Windows and fallback blocks
- [ ] Run `cargo check --package tama-cli`
  - Did it succeed? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test --package tama-cli`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "chore: strip Windows code from tama-cli"

**Acceptance criteria:**
- [ ] `service.rs` does not exist
- [ ] No `#[cfg(windows)]` or `#[cfg(target_os = "windows")]` in any tama-cli `.rs` file
- [ ] No `#[cfg(not(any(target_os = "windows", target_os = "linux")))]` in any tama-cli `.rs` file
- [ ] `cargo check --package tama-cli` passes
- [ ] `cargo test --package tama-cli` passes

---

## Task 4: Strip Windows code from tama-web

**Context:**
Remove the Windows branch from tama-web's jobs.rs kill_children function.

**Files:**
- Modify: `crates/tama-web/src/jobs.rs`

**What to implement:**

In `jobs.rs`:
- Remove the entire `#[cfg(windows)]` block (lines ~252-262) including the `use std::os::windows::process::CommandExt` import and `creation_flags(0x00000008)` call
- Make the `#[cfg(unix)]` block unconditional (remove the `#[cfg(unix)]` gate)
- Update the comment at line 2 — change "Use std::process::kill for Unix, taskkill for Windows" to just "Kill child processes by PID"

**Steps:**
- [ ] Edit `crates/tama-web/src/jobs.rs` — remove Windows block, make Unix block unconditional, update comment
- [ ] Run `cargo check --package tama-web`
  - Did it succeed? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "chore: strip Windows code from tama-web"

**Acceptance criteria:**
- [ ] No `#[cfg(windows)]` in tama-web
- [ ] `cargo check --package tama-web` passes
- [ ] `cargo test --package tama-web` passes

---

## Task 5: Remove Windows dependencies from Cargo.toml files

**Context:**
Remove the `[target.'cfg(windows)'.dependencies]` sections from both tama-core and tama-cli. Remove zip-related features from self_update in tama-core (they're only used for Windows archive extraction).

**Files:**
- Modify: `crates/tama-core/Cargo.toml`
- Modify: `crates/tama-cli/Cargo.toml`

**What to implement:**

1. **`crates/tama-core/Cargo.toml`** — Remove the entire `[target.'cfg(windows)'.dependencies]` section (removes `windows-service` and `windows-sys`). From the `self_update` dependency line, remove `archive-zip` and `compression-zip-deflate` features (keep `archive-tar`, `compression-flate2`, `rustls`, `ureq`).

2. **`crates/tama-cli/Cargo.toml`** — Remove the entire `[target.'cfg(windows)'.dependencies]` section (removes `windows-service`).

**Steps:**
- [ ] Edit `crates/tama-core/Cargo.toml` — remove Windows dependency section, remove zip features from self_update
- [ ] Edit `crates/tama-cli/Cargo.toml` — remove Windows dependency section
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "chore: remove Windows dependencies from Cargo.toml"

**Acceptance criteria:**
- [ ] No `[target.'cfg(windows)'.dependencies]` in any Cargo.toml
- [ ] No `archive-zip` or `compression-zip-deflate` in self_update features
- [ ] `cargo check --workspace` passes

---

## Task 6: Remove Windows CI/release jobs and add grep guard

**Context:**
Remove Windows CI and release jobs from GitHub Actions. Add a grep step to CI that prevents `cfg(windows)` code from being re-introduced.

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/release.yml`

**What to implement:**

1. **`.github/workflows/ci.yml`** — Remove the `test-windows` job entirely. Add a new step in the `test` job (after "Format check") that runs:
   ```yaml
   - name: Check for Windows cfg code
     run: |
       if grep -rP 'cfg\s*\(.*windows' crates/ --include='*.rs' | grep -v '//.*[Ww]indows' | grep -v '#\[cfg(not(target_os = "linux"))\]'; then
         echo "ERROR: Found cfg(windows) code. Windows support has been removed."
         exit 1
       fi
   ```

2. **`.github/workflows/release.yml`** — Remove the `build-windows` job entirely. In the `release` job, change `needs: [version, build-windows, build-linux]` to `needs: [version, build-linux]`. Remove the "Download Windows artifacts" step. Remove Windows artifacts from the `files:` list (`windows/**/*.exe`, `windows/**/tama-x86_64-pc-windows-msvc.zip`). Remove Windows installation instructions from the release body (the "### Windows" section with installer and portable instructions, and "3. Install as a service (Windows)").

**Steps:**
- [ ] Edit `.github/workflows/ci.yml` — remove `test-windows` job, add grep guard step
- [ ] Edit `.github/workflows/release.yml` — remove `build-windows` job, update `release` job dependencies, remove Windows artifacts and instructions
- [ ] Commit with message: "chore: remove Windows CI/release jobs, add cfg guard"

**Acceptance criteria:**
- [ ] No `test-windows` or `build-windows` job in any workflow
- [ ] CI has a grep step that catches `cfg(windows)` code
- [ ] Release workflow only depends on `build-linux`
- [ ] Release body has no Windows installation instructions

---

## Task 7: Update Makefile and README.md

**Context:**
Remove Windows-specific Makefile targets and update README to reflect Linux-only support.

**Files:**
- Modify: `Makefile`
- Modify: `README.md`

**What to implement:**

1. **`Makefile`** — Remove the `install-global` target. Remove the `build-windows` target. Remove `build-windows` from the `check` target's dependencies (change `check: fmt-check clippy test build-windows` to `check: fmt-check clippy test`).

2. **`README.md`** — Update the "Cross-platform" feature bullet to "Linux support with native systemd integration". Remove Windows installation instructions (installer, portable `.exe`, service install). Remove `%APPDATA%` path references. Remove `tama.exe` references in build section.

**Steps:**
- [ ] Edit `Makefile` — remove `install-global`, `build-windows` targets, and `build-windows` from `check`
- [ ] Edit `README.md` — update cross-platform claim, remove Windows instructions and references
- [ ] Commit with message: "chore: update Makefile and README for Linux-only"

**Acceptance criteria:**
- [ ] No `build-windows` or `install-global` targets in Makefile
- [ ] README has no Windows installation instructions
- [ ] README no longer claims cross-platform support

---

## Task 8: Final verification

**Context:**
Run the full verification suite to ensure everything builds, lints, and tests correctly.

**Steps:**
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo build --release --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `rg 'cfg.*windows' crates/ --type rust -n` and verify no false positives remain (only comments about future Windows support should appear)
- [ ] Commit any fixes with message: "fix: resolve remaining Windows removal issues"

**Acceptance criteria:**
- [ ] `cargo fmt --all` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo build --release --workspace` passes
- [ ] No `cfg(windows)` code remains in source files
