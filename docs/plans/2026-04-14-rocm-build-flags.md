# ROCm Build Flags for llama.cpp / ik_llama Source Builds — Plan

**Goal:** When koji builds llama.cpp or ik_llama from source with `GpuType::RocM`, emit the full set of recommended ROCm/HIP cmake flags (detected `AMDGPU_TARGETS`, rocWMMA flash attention, all KV-cache FA quants, libcurl) and export `HIPCXX`/`HIP_PATH` so the HIP toolchain is picked up reliably.

**Architecture:** Detection lives in `crates/koji-core/src/gpu.rs` as a pure function that shells out to `rocminfo` (with a `KOJI_AMDGPU_TARGETS` env override). Flag assembly stays in `build_cmake_args` in `crates/koji-core/src/backends/installer/source.rs` — the function's signature grows an `amdgpu_targets: Option<&[String]>` parameter so it stays pure and unit-testable. The Linux cmake subprocess gets `HIPCXX`/`HIP_PATH` from `hipconfig` injected into its env at the call site in `configure_cmake`.

**Tech Stack:** Rust, tokio (subprocess), std::process::Command (sync), existing cmake build pipeline. No new crate deps.

---

## Background (non-normative)

Current state in `crates/koji-core/src/backends/installer/source.rs:331-333` (inside `build_cmake_args` which starts at line 306):

```rust
GpuType::RocM { .. } => {
    cmake_args.push("-DGGML_HIP=ON".to_string());
}
```

That's the only ROCm flag emitted. Gaps (verified against llama.cpp `docs/build.md`, `ggml/src/ggml-hip/CMakeLists.txt`, AMD's ROCm install guide, and discussion #15021):

- No `AMDGPU_TARGETS` — llama.cpp's default target list varies by ROCm version and often excludes RDNA4 (gfx1200/gfx1201) and other newer archs, producing a build that silently won't run on the installed GPU.
- No `GGML_HIP_ROCWMMA_FATTN=ON` — Flash Attention via rocWMMA is a major PP speedup on RDNA3+/RDNA4 and is supported from ROCm ≥ 6.1. Leave enabled unconditionally: the rocWMMA headers ship with ROCm ≥ 6.1, and on CDNA it is neutral-to-slightly-negative but not broken.
- No `GGML_CUDA_FA_ALL_QUANTS=ON` — without this, `-ctk q8_0`/similar at runtime falls back to F16. This is the same rationale that already drives the `ik_llama` block at `source.rs:349`.
- No `LLAMA_CURL=ON` — AMD's install guide enables it so `llama-server -hf …` pulls work.
- `hipconfig`-derived `HIPCXX`/`HIP_PATH` not exported — if `/opt/rocm/bin` isn't on `PATH`, HIP discovery fails or picks the wrong toolchain.

Not in scope (out-of-scope, do not attempt):
- Adding a `gfx_target` field to `GpuType::RocM`. Detection and env override are enough today; a schema change requires DB migration + UI wiring that belongs to a separate plan.
- Adding `HSA_OVERRIDE_GFX_VERSION` runtime env for backend processes (workaround for the ROCm 7.2 regression on gfx1201). That's a runtime env, not a build flag — separate plan.
- Windows ROCm support. AMD ships ROCm for Linux; Windows HIP builds are a different codepath.
- Gating rocWMMA by arch. It is enabled unconditionally per above; revisit only if a user reports a regression.

---

### Task 1: Add `detect_amdgpu_targets()` to `gpu.rs`

**Context:**
This task introduces a pure Rust helper that, on Linux, shells out to `rocminfo` and returns the unique list of `gfxNNNN` architecture names for the GPUs visible to the system. It also respects an env override `KOJI_AMDGPU_TARGETS` (semicolon- or comma-separated string) for headless/container builds where `rocminfo` can't see the GPU. The function is pure aside from the env read and subprocess — it does not mutate state or touch the filesystem. It returns `Vec<String>` (empty if detection fails or rocminfo is unavailable). This helper will be consumed by Task 2.

**Files:**
- Modify: `crates/koji-core/src/gpu.rs`

**What to implement:**

Add a new public function:

```rust
/// Detect AMD GPU architectures suitable for `-DAMDGPU_TARGETS=...`.
///
/// Honors `KOJI_AMDGPU_TARGETS` as an override (accepts `;` or `,` as
/// separators; whitespace trimmed; empty entries dropped). Otherwise runs
/// `rocminfo` and parses `Name:\s+gfx[0-9a-f]+` lines. Returns the
/// deduplicated list in first-seen order. Returns an empty `Vec` if
/// detection fails, `rocminfo` is unavailable, or no gfx entries are found.
///
/// This function is Linux-oriented but compiles on all platforms — on
/// non-Linux hosts it returns `Vec::new()` unless the env override is set.
pub fn detect_amdgpu_targets() -> Vec<String>
```

Plus a pure `parse_rocminfo_gfx_names(stdout: &str) -> Vec<String>` helper that the tests can exercise without running `rocminfo`.

Parsing rules:
- Match lines whose trimmed content begins with `Name:` followed by whitespace and a token that starts with `gfx` and is followed by one or more hexadecimal digits (lowercase).
- Regex-free: use `str::split_whitespace` after trimming the `Name:` prefix. Avoid pulling in the `regex` crate.
- Deduplicate preserving first-seen order (e.g. collect into `Vec<String>` while tracking a small `HashSet<String>`).
- Ignore any `Name:` line whose value does not match the `gfx<hex+>` shape — `rocminfo` also prints CPU entries (e.g. `Name: AMD Ryzen …`).

Env override rules:
- Read `KOJI_AMDGPU_TARGETS` via `std::env::var`. If present and non-empty after trim, split on `,` or `;`, trim each piece, drop empty pieces, return the resulting `Vec<String>` without running `rocminfo`. Do not validate the shape — the user is asserting they know what they want.
- If the env var is unset or empty, fall through to `rocminfo`.

**Steps:**
- [ ] Write failing tests in the existing `#[cfg(test)] mod tests` block at the bottom of `crates/koji-core/src/gpu.rs`.

  Because Rust's default test harness is multi-threaded and the env-override tests mutate a shared process-global env var (`KOJI_AMDGPU_TARGETS`), serialize them with a `static Mutex<()>` in the test module. Do NOT add the `serial_test` crate — stay dep-free. Put this near the top of `mod tests`:

  ```rust
  use std::sync::Mutex;
  static ENV_LOCK: Mutex<()> = Mutex::new(());
  ```

  Each env-override test must take `let _guard = ENV_LOCK.lock().unwrap();` as its first line, and each must `std::env::remove_var("KOJI_AMDGPU_TARGETS")` at its start (defensive, in case a prior run panicked) and also at its end.

  Tests to add:
  - `test_parse_rocminfo_gfx_names_single_gpu`: input containing one CPU `Name: AMD Ryzen …` and one GPU `Name:  gfx1201` → returns `vec!["gfx1201"]`.
  - `test_parse_rocminfo_gfx_names_multi_gpu_dedup`: input with `Name: gfx1100` and `Name: gfx1201` and a repeat `Name: gfx1100` → returns `vec!["gfx1100", "gfx1201"]` (order preserved, no duplicate).
  - `test_parse_rocminfo_gfx_names_no_match`: input with only CPU Name lines → returns empty vec.
  - `test_parse_rocminfo_gfx_names_tolerates_trailing_whitespace`: input with `  Name:   gfx942   \n` → returns `vec!["gfx942"]`.
  - `test_detect_amdgpu_targets_env_override_semicolons` (uses `ENV_LOCK`): `std::env::set_var("KOJI_AMDGPU_TARGETS", "gfx1100;gfx1201")` then call → returns `vec!["gfx1100", "gfx1201"]`. Clean up with `remove_var`.
  - `test_detect_amdgpu_targets_env_override_commas_and_whitespace` (uses `ENV_LOCK`): `"  gfx942 , gfx90a "` → returns `vec!["gfx942", "gfx90a"]`. Clean up with `remove_var`.
  - `test_detect_amdgpu_targets_env_override_empty_is_ignored` (uses `ENV_LOCK`): set to `""` then call → env path must be skipped and behavior must fall through to rocminfo. Assertion: `result.is_empty() || result.iter().all(|s| s.starts_with("gfx"))` (tolerant — passes on CI where rocminfo is absent and on dev boxes where it is present). Clean up with `remove_var` at the end.
- [ ] Run `cargo test -p koji-core --lib gpu::tests::test_parse_rocminfo_gfx_names_single_gpu`
  - Did it fail with "function not found" / "unresolved import"? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `parse_rocminfo_gfx_names` and `detect_amdgpu_targets` in `crates/koji-core/src/gpu.rs`. Place them after `detect_cuda_version_nvidia_smi` (around line 325) to keep vendor-detection helpers grouped. Add brief rustdoc per the signature above.
- [ ] Run `cargo test -p koji-core --lib gpu::tests`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo fmt`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy -p koji-core --lib -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Run `cargo build -p koji-core`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: `feat(gpu): add detect_amdgpu_targets() for ROCm build target detection`

**Acceptance criteria:**
- [ ] `pub fn detect_amdgpu_targets() -> Vec<String>` exported from `koji_core::gpu`.
- [ ] `pub fn parse_rocminfo_gfx_names(stdout: &str) -> Vec<String>` exported (pub so tests in other crates could consume it later; not strictly required but lets the webapp DTO layer reuse it if needed).
- [ ] All new unit tests pass; no existing test regresses.
- [ ] Env override splits on both `,` and `;`, trims whitespace, drops empties.
- [ ] No new crate dependencies added to `Cargo.toml`.

---

### Task 2: Extend `build_cmake_args` with full ROCm flag set

**Context:**
`build_cmake_args` in `crates/koji-core/src/backends/installer/source.rs` currently emits only `-DGGML_HIP=ON` for the ROCm branch. This task extends the ROCm branch to emit `-DAMDGPU_TARGETS=<detected>`, `-DGGML_HIP_ROCWMMA_FATTN=ON`, `-DGGML_CUDA_FA_ALL_QUANTS=ON`, and `-DLLAMA_CURL=ON`. To keep `build_cmake_args` pure (it is deliberately extracted for testability — see the comment at `source.rs:303-305`), detection is performed by the caller and passed in. The function signature gains `amdgpu_targets: &[String]`.

**Files:**
- Modify: `crates/koji-core/src/backends/installer/source.rs`

**What to implement:**

1. Change the signature of `build_cmake_args` from

   ```rust
   fn build_cmake_args(
       options: &InstallOptions,
       source_dir: &Path,
       build_output: &Path,
   ) -> Vec<String>
   ```

   to

   ```rust
   fn build_cmake_args(
       options: &InstallOptions,
       source_dir: &Path,
       build_output: &Path,
       amdgpu_targets: &[String],
   ) -> Vec<String>
   ```

2. Inside the `GpuType::RocM { .. } => { … }` branch:

   ```rust
   cmake_args.push("-DGGML_HIP=ON".to_string());
   cmake_args.push("-DGGML_HIP_ROCWMMA_FATTN=ON".to_string());
   cmake_args.push("-DGGML_CUDA_FA_ALL_QUANTS=ON".to_string());
   cmake_args.push("-DLLAMA_CURL=ON".to_string());
   if !amdgpu_targets.is_empty() {
       cmake_args.push(format!("-DAMDGPU_TARGETS={}", amdgpu_targets.join(";")));
   }
   ```

   `;` is the separator used by CMake for list-valued cache variables and matches AMD's install guide.

3. Update the single non-test caller, `configure_cmake` at `source.rs:509-537`, to detect targets and pass them in:

   ```rust
   let amdgpu_targets = if matches!(options.gpu_type, Some(GpuType::RocM { .. })) {
       let targets = crate::gpu::detect_amdgpu_targets();
       if targets.is_empty() {
           tracing::warn!(
               "No AMDGPU_TARGETS detected (rocminfo missing or returned no gfx entries). \
                Falling back to llama.cpp's default target list — this may exclude newer archs. \
                Set KOJI_AMDGPU_TARGETS=gfxNNNN to override."
           );
       } else {
           tracing::info!("Detected AMDGPU_TARGETS: {}", targets.join(";"));
       }
       targets
   } else {
       Vec::new()
   };
   let cmake_args = build_cmake_args(options, source_dir, build_output, &amdgpu_targets);
   ```

4. Update the four existing unit tests in `source.rs` (lines ~559-640) that call `build_cmake_args` to pass an empty slice `&[]` as the new argument. Do not change their assertions.

**What NOT to change:**
- Do not modify the CUDA, Vulkan, Metal, CpuOnly, or Custom branches.
- Do not touch the ik_llama-specific block (`GGML_IQK_FA_ALL_QUANTS`, Windows Ninja+clang-cl setup).
- Do not add a `gfx_target` field to `GpuType::RocM`.
- Do not change the ROCm URL hardcoding in `urls.rs:37` (separate plan).
- Do not enable `GGML_HIP_UMA`, `GGML_HIP_RCCL`, `GGML_HIP_NO_VMM`, or `GGML_HIP_GRAPHS`. Those are situational (APU-only / multi-GPU / workarounds).

**Steps:**
- [ ] Write failing tests in the `mod tests` block at the bottom of `crates/koji-core/src/backends/installer/source.rs`:
  - `test_rocm_emits_full_flag_set`: `make_options(BackendType::LlamaCpp, Some(GpuType::RocM { version: "7.2".to_string() }))`, call with `&["gfx1201".to_string()]`, assert the returned `Vec<String>` contains all of `-DGGML_HIP=ON`, `-DGGML_HIP_ROCWMMA_FATTN=ON`, `-DGGML_CUDA_FA_ALL_QUANTS=ON`, `-DLLAMA_CURL=ON`, and `-DAMDGPU_TARGETS=gfx1201`.
  - `test_rocm_multi_target_joined_with_semicolons`: call with `&["gfx1100".to_string(), "gfx1201".to_string()]`, assert args contains `-DAMDGPU_TARGETS=gfx1100;gfx1201`.
  - `test_rocm_no_targets_omits_amdgpu_targets_flag`: call with `&[]`, assert no arg starts with `-DAMDGPU_TARGETS=`, but the other four ROCm flags are still present.
  - `test_non_rocm_never_emits_rocm_flags`: `make_options(BackendType::LlamaCpp, Some(GpuType::Cuda { version: "12".to_string() }))`, call with `&["gfx1201".to_string()]` (even with targets accidentally passed), assert none of `-DGGML_HIP=ON`, `-DGGML_HIP_ROCWMMA_FATTN=ON`, `-DAMDGPU_TARGETS=…` appear. (Defensive: ensures `amdgpu_targets` is ignored outside the ROCm branch.)
  - `test_ik_llama_rocm_includes_both_iqk_and_rocwmma`: `make_options(BackendType::IkLlama, Some(GpuType::RocM { … }))`, call with `&["gfx942".to_string()]`, assert args contains both `-DGGML_IQK_FA_ALL_QUANTS=ON` (ik_llama block) and `-DGGML_HIP_ROCWMMA_FATTN=ON` (ROCm block).
- [ ] Run `cargo test -p koji-core --lib backends::installer::source::tests::test_rocm_emits_full_flag_set`
  - Did it fail with a compile error (missing arg) or assertion failure? If it passed unexpectedly, stop and investigate.
- [ ] Update the `build_cmake_args` signature and ROCm branch body as specified above.
- [ ] Update the four existing test call sites (`test_ik_llama_includes_iqk_fa_all_quants`, `test_llama_cpp_excludes_iqk_fa_all_quants`, `test_ik_llama_cuda_includes_both_flags`, `test_ik_llama_windows_uses_ninja_clang_cl_avx2`) to pass `&[]` as the fourth argument.
- [ ] Update `configure_cmake` to detect targets and pass them in (see snippet above).
- [ ] Run `cargo test -p koji-core --lib backends::installer::source::tests`
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy -p koji-core --lib -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: `feat(backends): emit full ROCm flag set (AMDGPU_TARGETS, rocWMMA FA, FA_ALL_QUANTS, LLAMA_CURL)`

**Acceptance criteria:**
- [ ] `build_cmake_args` signature takes `amdgpu_targets: &[String]`.
- [ ] ROCm branch emits all five flags (`GGML_HIP`, `GGML_HIP_ROCWMMA_FATTN`, `GGML_CUDA_FA_ALL_QUANTS`, `LLAMA_CURL`, and `AMDGPU_TARGETS` when non-empty).
- [ ] `configure_cmake` calls `detect_amdgpu_targets()` only when `gpu_type` is ROCm.
- [ ] Empty-targets path logs a warning; non-empty path logs an info line with the target list.
- [ ] All existing and new unit tests pass.
- [ ] No regressions in CUDA/Vulkan/Metal branches.

---

### Task 3: Export `HIPCXX` and `HIP_PATH` for the Linux cmake invocation

**Context:**
AMD's official install guide and llama.cpp's build doc both prepend `HIPCXX="$(hipconfig -l)/clang" HIP_PATH="$(hipconfig -R)"` to the cmake command. This ensures cmake's HIP language support finds the right clang frontend even when `/opt/rocm/bin` isn't on `PATH`. Koji currently invokes cmake without any env injection (`source.rs:524-527`). This task adds a helper that queries `hipconfig` once, then sets those two env vars on the `tokio::process::Command` used to run cmake — but only when `gpu_type` is `RocM` and only on non-Windows targets.

**Files:**
- Modify: `crates/koji-core/src/backends/installer/source.rs`

**What to implement:**

1. Add a pure helper that assembles the env pair from two already-captured strings, and a thin wrapper that shells out to `hipconfig`. Splitting them lets us unit-test the parsing without a subprocess.

   ```rust
   #[cfg(not(target_os = "windows"))]
   fn hip_env_from_hipconfig_output(clang_dir_stdout: &str, hip_root_stdout: &str) -> Option<(String, String)> {
       let clang_dir = clang_dir_stdout.trim();
       let hip_root = hip_root_stdout.trim();
       if clang_dir.is_empty() || hip_root.is_empty() {
           return None;
       }
       Some((format!("{}/clang", clang_dir), hip_root.to_string()))
   }

   #[cfg(not(target_os = "windows"))]
   fn detect_hip_env() -> Option<(String, String)> {
       // Runs `hipconfig -l` and `hipconfig -R`. Returns None if hipconfig is
       // unavailable, either call fails, or either stdout is empty.
       let clang_dir = std::process::Command::new("hipconfig")
           .arg("-l")
           .output()
           .ok()?;
       if !clang_dir.status.success() {
           return None;
       }
       let hip_root = std::process::Command::new("hipconfig")
           .arg("-R")
           .output()
           .ok()?;
       if !hip_root.status.success() {
           return None;
       }
       hip_env_from_hipconfig_output(
           &String::from_utf8_lossy(&clang_dir.stdout),
           &String::from_utf8_lossy(&hip_root.stdout),
       )
   }
   ```

2. In the `#[cfg(not(target_os = "windows"))]` branch of `configure_cmake` (`source.rs:522-536`), before invoking the command, add:

   ```rust
   let mut cmd = tokio::process::Command::new("cmake");
   cmd.args(&cmake_args);
   if matches!(options.gpu_type, Some(GpuType::RocM { .. })) {
       if let Some((hipcxx, hip_path)) = detect_hip_env() {
           tracing::info!("Using HIPCXX={}, HIP_PATH={}", hipcxx, hip_path);
           cmd.env("HIPCXX", hipcxx);
           cmd.env("HIP_PATH", hip_path);
       } else {
           tracing::warn!(
               "hipconfig not found or returned empty output. \
                Falling back to PATH-based HIP discovery. \
                Ensure /opt/rocm/bin is on PATH if the build fails."
           );
       }
   }
   let status = cmd.status().await?;
   ```

   Replace the existing direct `tokio::process::Command::new("cmake").args(&cmake_args).status().await?` call with this block.

**What NOT to change:**
- Do not touch the Windows `configure_cmake_windows` path — ROCm on Windows is out of scope for this plan.
- Do not export `HIPCXX`/`HIP_PATH` for non-ROCm builds.
- Do not change how the build step (`build_cmake` at the subsequent `cmake --build`) is invoked. The env is only needed at configure time; cmake caches the compiler selection into `CMakeCache.txt`.

**Steps:**
- [ ] Write failing tests in the `mod tests` block of `crates/koji-core/src/backends/installer/source.rs`. Wrap each test body with `#[cfg(not(target_os = "windows"))]` to match the helper's cfg-gate:
  - `test_hip_env_from_hipconfig_output_happy_path`: input `("/opt/rocm/llvm/bin\n", "/opt/rocm\n")` → returns `Some(("/opt/rocm/llvm/bin/clang".to_string(), "/opt/rocm".to_string()))`.
  - `test_hip_env_from_hipconfig_output_empty_stdout_returns_none`: input `("", "/opt/rocm")` → `None`; input `("/opt/rocm/llvm/bin", "   ")` → `None`.
  - `test_hip_env_from_hipconfig_output_trims_whitespace`: input `("  /opt/rocm/llvm/bin  \n", "\t/opt/rocm\t\n")` → `Some(("/opt/rocm/llvm/bin/clang".to_string(), "/opt/rocm".to_string()))`.
- [ ] Run `cargo test -p koji-core --lib backends::installer::source::tests::test_hip_env_from_hipconfig_output_happy_path`
  - Did it fail with "function not found"? If it passed unexpectedly, stop and investigate.
- [ ] Add the `hip_env_from_hipconfig_output` and `detect_hip_env` helpers under the existing `#[cfg(not(target_os = "windows"))]` blocks in `source.rs`.
- [ ] Modify `configure_cmake` non-Windows branch as specified.
- [ ] Run `cargo test -p koji-core --lib`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy -p koji-core --lib -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Manual smoke test (optional — only if an AMD GPU machine is available): run `koji backend install llama_cpp --gpu rocm --build` and confirm the cmake configure log shows the env vars and the resulting `CMakeCache.txt` references `/opt/rocm/llvm/bin/clang` or equivalent under `CMAKE_HIP_COMPILER`. Document the result in the commit message.
- [ ] Commit with message: `feat(backends): export HIPCXX and HIP_PATH for ROCm cmake configure on Linux`

**Acceptance criteria:**
- [ ] `detect_hip_env()` returns `None` when `hipconfig` is missing; returns `Some((hipcxx, hip_path))` otherwise.
- [ ] `configure_cmake` sets the two env vars on the cmake subprocess only for ROCm builds on Linux.
- [ ] Warning is logged when `hipconfig` is unavailable; info line logs the resolved paths otherwise.
- [ ] No behavior change for CUDA/Vulkan/Metal/CPU or for Windows builds.
- [ ] All existing tests still pass.

---

### Task 4: Update `docs/plans/README.md`

**Context:**
The plans README is the index of every plan in the repo. New plans must be registered there with status 🚧 IN PROGRESS, moving to ✅ COMPLETED once merged. The Quick Stats line at the top also needs its counts bumped.

**Files:**
- Modify: `docs/plans/README.md`

**What to implement:**

1. Under the **Backend Management** table (the same section that lists `2026-04-08-backends-install-update-ui-spec.md` and `2026-04-10-fix-backend-default-args-spec.md`), add a new row:

   ```markdown
   | [ROCm Build Flags](2026-04-14-rocm-build-flags.md) | Detect AMDGPU_TARGETS via rocminfo; add rocWMMA FA, FA_ALL_QUANTS, LLAMA_CURL; export HIPCXX/HIP_PATH | 🚧 IN PROGRESS |
   ```

   If the existing Backend Management table does not have a status column, match whatever column layout it uses — do not restructure the table.

2. Update the Quick Stats block (`## Quick Stats`, currently showing `Total Plans: 52`, `Completed: 50 ✅`):
   - Increment `Total Plans` by 1 (from whatever the current value is).
   - Leave `Completed` unchanged.
   - Leave the `Remaining` list unchanged (this plan is in-progress, not "not started").

**Steps:**
- [ ] Open `docs/plans/README.md` and confirm the current Total Plans count and the exact Backend Management table structure.
- [ ] Add the new row to the Backend Management table.
- [ ] Bump Total Plans.
- [ ] Commit with message: `docs(plans): register ROCm build flags plan`

**Acceptance criteria:**
- [ ] New row appears in Backend Management table linking to `2026-04-14-rocm-build-flags.md`.
- [ ] Total Plans count incremented.
- [ ] Markdown renders without broken tables (spot-check via `git diff` — column counts match).

---

## Verification checklist (whole plan)

After all four tasks are committed:

- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace -- -D warnings` passes.
- [ ] `cargo fmt --check` is clean.
- [ ] Grep `rg -n "GGML_HIP=ON" crates/koji-core/src/backends/installer/source.rs` shows the flag only inside the `GpuType::RocM` branch.
- [ ] Grep `rg -n "AMDGPU_TARGETS" crates/koji-core/src/backends/installer/source.rs` shows the `format!` inside the ROCm branch.
- [ ] `rg -n "detect_amdgpu_targets" crates/koji-core` finds the definition in `gpu.rs` and the call in `source.rs`.
- [ ] `rg -n "HIPCXX" crates/koji-core` finds the env injection in `configure_cmake`.
- [ ] The plan entry in `docs/plans/README.md` links to the correct file and is in the Backend Management section.
