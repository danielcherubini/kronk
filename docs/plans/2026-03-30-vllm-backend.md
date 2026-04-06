# vLLM Backend Support Plan

**Goal:** Add vLLM as a first-class backend type in koji with full lifecycle management (start, stop, health check, auto-unload), matching the existing llama.cpp/ik_llama experience.

**Architecture:** vLLM is a Python-based inference server (installed via pip) that serves an OpenAI-compatible API on `/v1/chat/completions` with health checks on `/health`. Unlike llama.cpp, it uses HuggingFace safetensors models (not GGUF), takes different CLI flags (`vllm serve <model> --port N`), handles sampling parameters per-request (not as CLI flags), and primarily runs on Linux. The proxy layer requires no changes since it just forwards HTTP. The key work is in the backend type enum, arg builder, installer, and updater.

**Tech Stack:** Rust (koji-core, koji-cli), Python (vLLM external dependency), PyPI API for version checks

---

### Task 1: Add `BackendType::Vllm` variant and update all match arms

**Context:**
Every backend in koji has a `BackendType` enum variant in `registry_ops.rs`. Adding a new backend requires updating this enum plus every `match` expression that pattern-matches on it across the entire workspace. This is a foundational task — all subsequent tasks depend on the `Vllm` variant existing. The variant needs Display, FromStr, and Serialize/Deserialize support. The compiler will flag any exhaustive-match errors, but the agent must proactively find and update every match arm.

**Files:**
- Modify: `crates/koji-core/src/backends/registry/registry_ops.rs` — enum definition + Display + FromStr
- Modify: `crates/koji-core/src/backends/installer/urls.rs` — `get_prebuilt_url` match arm
- Modify: `crates/koji-core/src/backends/updater.rs` — `check_latest_version` match arm
- Modify: `crates/koji-cli/src/commands/backend.rs` — **5 separate match arms** (see details below) + `parse_backend_type()` function
- Test: existing tests in `registry_ops.rs`, `urls.rs`

**What to implement:**

1. Add `Vllm` variant to the `BackendType` enum (search for `pub enum BackendType`).
2. Update `Display` impl to map `Vllm` → `"vllm"`.
3. Update `FromStr` impl: add `"vllm" => Ok(BackendType::Vllm)` and update the error message to list `vllm` as supported.
4. Add `BackendType::Vllm` arm to `get_prebuilt_url` in `urls.rs` returning: `Err(anyhow!("vLLM is a Python package. Install it with: pip install vllm"))`.
5. Add PyPI version checking for vLLM. In `updater.rs`:
   - Add structs:
     ```rust
     #[derive(Debug, Deserialize)]
     struct PypiPackage { info: PypiInfo }
     #[derive(Debug, Deserialize)]
     struct PypiInfo { version: String }
     ```
   - Add `BackendType::Vllm` arm in `check_latest_version` that:
     - GETs `https://pypi.org/pypi/vllm/json`
     - Deserializes as `PypiPackage`
     - Returns `Ok(commit.info.version)`

6. **In `crates/koji-cli/src/commands/backend.rs`**, update these 5 locations (search for each):
   - **`parse_backend_type()` function** (~line 80): Add `"vllm" => Ok(BackendType::Vllm)` and update the error message.
   - **`use_source` match** (search for `let use_source = match backend_type`): Add `BackendType::Vllm` arm that prints `"vLLM is a Python package, not built from source. Registering existing installation..."` and sets `use_source = false`. (The actual vLLM install flow will be built in Task 3; for now this prevents compiler errors.)
   - **`backend_name` match** (search for `let type_str = match backend_type`): Add `BackendType::Vllm => "vllm"`.
   - **`git_url` match** (search for `let git_url = match backend_type`): Add `BackendType::Vllm => return Err(anyhow!("vLLM does not use git-based installation"))`. This arm should bail early since the git_url/source construction below doesn't apply.
   - **`cmd_update` fallback match** (search for `match backend_info.backend_type` inside `cmd_update`): Add `BackendType::Vllm => return Err(anyhow!("Update vLLM with: pip install --upgrade vllm"))`.

7. Add a test `test_vllm_prebuilt_not_available` in `urls.rs` tests.

**Steps:**
- [ ] Add `Vllm` variant to `BackendType` enum in `crates/koji-core/src/backends/registry/registry_ops.rs`
- [ ] Update `Display` and `FromStr` impls in the same file
- [ ] Add `BackendType::Vllm` arm to `get_prebuilt_url` in `crates/koji-core/src/backends/installer/urls.rs`
- [ ] Add `PypiPackage`/`PypiInfo` structs and `BackendType::Vllm` arm to `check_latest_version` in `crates/koji-core/src/backends/updater.rs`
- [ ] Update all 5 match arms + `parse_backend_type()` in `crates/koji-cli/src/commands/backend.rs`
- [ ] Add test `test_vllm_prebuilt_not_available` in `urls.rs`
- [ ] Run `cargo test --workspace` — compiler will catch any missed match arms. Fix ALL exhaustiveness errors.
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat: add BackendType::Vllm variant with PyPI version checking`

**Acceptance criteria:**
- [ ] `BackendType::Vllm` exists and round-trips through Display/FromStr
- [ ] `"vllm".parse::<BackendType>()` returns `Ok(BackendType::Vllm)` (via `FromStr`)
- [ ] `parse_backend_type("vllm")` returns `Ok(BackendType::Vllm)`
- [ ] `get_prebuilt_url` returns a helpful error for Vllm
- [ ] `check_latest_version` for Vllm queries PyPI
- [ ] ALL `match BackendType` arms are handled (zero compiler warnings)
- [ ] All existing tests pass, no clippy warnings

---

### Task 2: Add vLLM-specific arg builder in `resolve.rs`

**Context:**
Koji builds CLI arguments for backends in `Config::build_full_args()` in `crates/koji-core/src/config/resolve.rs`. Currently this function assumes all backends use llama.cpp-style flags (`-m`, `-c`, `-ngl`, `--temp`, etc.). vLLM uses completely different flags. We need to detect when a backend is vLLM-type and build args differently.

To detect vLLM backends, we add an optional `backend_type` field to `BackendConfig`. When `backend_type` is `Some("vllm")`, the arg builder switches to vLLM mode. When `None`, it defaults to llama.cpp-style (backward compatible with all existing configs).

vLLM launch command format: `vllm serve <model> --max-model-len 32768 [other args]`
(Note: `--host` and `--port` are added by `lifecycle.rs` after `build_full_args()` returns, so do NOT add them here.)

Key differences from llama.cpp:
- First arg must be `serve` (vllm subcommand)
- Model is the second positional arg (HF model ID or local path)
- Context length flag is `--max-model-len` (not `-c`)
- No `-ngl`, `-m` flags
- Sampling parameters are per-request, NOT CLI flags — do not emit `--temp`, `--top-k`, etc.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs` — add `backend_type` field to `BackendConfig`
- Modify: `crates/koji-core/src/config/resolve.rs` — add `build_vllm_args()`, update `build_full_args()`
- Modify: `crates/koji-core/src/config/loader.rs` — add `backend_type: None` to default `BackendConfig`
- Modify: `crates/koji-cli/tests/tests.rs` — add `backend_type: None` to test `BackendConfig` construction
- Test: add tests in `crates/koji-core/src/config/resolve.rs`

**What to implement:**

1. **Add `backend_type` to `BackendConfig`** in `types.rs` (search for `pub struct BackendConfig`):
   ```rust
   #[serde(default)]
   pub backend_type: Option<String>,
   ```
   **IMPORTANT:** This field uses `#[serde(default)]` so existing TOML configs without it deserialize correctly (defaults to `None`). However, you must ALSO update every place that constructs a `BackendConfig` with struct literal syntax, adding `backend_type: None`. Search the entire workspace for `BackendConfig {` to find all construction sites:
   - `crates/koji-core/src/config/loader.rs` (in `Config::default()`)
   - `crates/koji-cli/tests/tests.rs` (in test helper)
   - Any other places found by the compiler

2. **Add `build_vllm_args()` method** to `Config` in `resolve.rs`:
   ```rust
   fn build_vllm_args(
       &self,
       server: &ModelConfig,
       backend: &BackendConfig,
       ctx_override: Option<u32>,
   ) -> Result<Vec<String>>
   ```
   This method:
   - Starts with `vec!["serve".to_string()]`
   - If `server.model` is `Some(model_id)`, push the model_id as the next positional arg
   - Appends all of `backend.default_args`
   - Appends all of `server.args`
   - Resolves context length: `ctx_override.or(server.context_length)`. If `Some(ctx)`, and `--max-model-len` is NOT already in args, add `["--max-model-len", ctx.to_string()]`
   - Does NOT add any sampling params (`--temp`, `--top-k`, etc.)
   - Does NOT add `-m`, `-c`, `-ngl`
   - Does NOT add `--host` or `--port` (lifecycle.rs handles these)
   - Returns `Ok(args)`

3. **Update `build_full_args()`** — at the **very top** of the method body (before the existing `let mut args = self.build_args(...)` line), add an early-return check:
   ```rust
   // vLLM backends use completely different CLI args — delegate and return early
   if backend.backend_type.as_deref() == Some("vllm") {
       return self.build_vllm_args(server, backend, ctx_override);
   }
   ```
   This bypasses ALL the llama.cpp-specific logic (GGUF path resolution, `-m`, `-c`, `-ngl`, sampling flags).

4. **Add tests** in a `#[cfg(test)] mod tests` block at the bottom of `resolve.rs` (create it if it doesn't exist):
   - `test_build_vllm_args_basic`: Create a `Config` with a model that has `model = Some("org/model")` and a backend with `backend_type = Some("vllm")`. Call `build_full_args()` and verify args start with `["serve", "org/model"]`.
   - `test_build_vllm_args_with_context`: Set `context_length = Some(32768)`. Verify `--max-model-len` and `32768` are in args.
   - `test_build_vllm_args_no_sampling_flags`: Set a profile with sampling params. Verify no `--temp`, `--top-k`, `--top-p`, etc. appear in the vLLM args.
   - `test_build_full_args_non_vllm_unchanged`: Create a backend WITHOUT `backend_type` set. Verify `build_full_args` still produces llama.cpp-style args (backward compatibility).

**Steps:**
- [ ] Add `backend_type: Option<String>` field with `#[serde(default)]` to `BackendConfig` in `types.rs`
- [ ] Search workspace for `BackendConfig {` and add `backend_type: None` to every construction site
- [ ] Add `build_vllm_args()` method to `Config` in `resolve.rs`
- [ ] Add early-return check at top of `build_full_args()` in `resolve.rs`
- [ ] Add tests in `resolve.rs`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat: add vLLM-specific arg builder with serve subcommand and --max-model-len`

**Acceptance criteria:**
- [ ] vLLM backends produce args like `["serve", "org/model", "--max-model-len", "32768", ...]`
- [ ] No `-m`, `-c`, `-ngl`, or sampling flags appear in vLLM args
- [ ] `--max-model-len` is added when context length is configured
- [ ] Existing llama.cpp/ik_llama arg building is completely unchanged (backward compatible)
- [ ] Existing configs without `backend_type` field continue to work
- [ ] All tests pass

---

### Task 3: Add vLLM to CLI backend install command

**Context:**
The CLI `koji backend install` command in `crates/koji-cli/src/commands/backend.rs` handles backend installation. For llama.cpp it downloads prebuilt binaries or builds from source. For ik_llama it builds from source. For vLLM, installation is fundamentally different — it's a Python package installed via pip, not a compiled binary.

Phase 1 approach: **manual registration** where koji detects an existing `vllm` binary on the system. The user must have already installed vLLM via `pip install vllm`. Koji finds the binary, registers it in the backend registry, and writes the config. This skips the entire `InstallOptions`/`install_backend()` path that llama.cpp/ik_llama use.

For the backend registry, vLLM backends use `source: None` on `BackendInfo` (since they're not built from source or downloaded as prebuilt).

**Files:**
- Modify: `crates/koji-cli/src/commands/backend.rs` — main install flow for vLLM
- Modify: `crates/koji-core/src/config/loader.rs` — (optional) helper for adding vLLM backend config

**What to implement:**

1. **Restructure the install flow in `cmd_install()` in `backend.rs`**: After `parse_backend_type()` succeeds, add an early-exit branch for vLLM **before** the GPU selection, source/prebuilt selection, and `install_backend()` call:

   ```rust
   if matches!(backend_type, BackendType::Vllm) {
       return install_vllm(config, name, version).await;
   }
   ```

2. **Add `install_vllm()` function** in `backend.rs`:
   ```rust
   async fn install_vllm(
       config: &mut Config,
       name: Option<String>,
       _version: Option<String>,
   ) -> Result<()>
   ```
   This function:
   - **Platform check**: If `cfg!(target_os = "windows")`, bail with `"vLLM is only available on Linux. See: https://docs.vllm.ai/en/latest/getting_started/installation.html"`.
   - **Find the binary**: Run `which vllm` via `tokio::process::Command`. If it fails, bail with `"vLLM not found on PATH. Install it with: pip install vllm"`.
   - **Get the version**: Run `vllm version` via `tokio::process::Command`, capture stdout. Parse the version string (trim whitespace). If this fails, use `"unknown"` as the version.
   - **Determine backend name**: Use `name.unwrap_or_else(|| format!("vllm_{}", version))`.
   - **Register in backend registry**: Load the `BackendRegistry`, call `registry.add_unchecked(BackendInfo { name, backend_type: BackendType::Vllm, version, path: vllm_path, installed_at: now, gpu_type: None, source: None })` and save.
   - **Add to config**: Insert a `BackendConfig` into `config.backends` with `path` set to the detected vllm binary, `backend_type: Some("vllm".to_string())`, `default_args: vec!["--dtype".to_string(), "auto".to_string()]`, `health_check_url: None`. Save the config.
   - Print success message with the detected path and version.

3. **Update the interactive backend type prompt** (search for `inquire::Select::new` with `"Select a backend type"` in `cmd_install`): Add `"vLLM"` to the options list, and map it to `BackendType::Vllm`.

**Steps:**
- [ ] Add platform check for vLLM (Linux only)
- [ ] Implement `install_vllm()` function with `which` detection and registry registration
- [ ] Add early-exit branch in `cmd_install()` for vLLM before GPU/source selection
- [ ] Update interactive backend type selection to include vLLM
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat: add vLLM backend install with auto-detection from PATH`

**Acceptance criteria:**
- [ ] `koji backend install vllm` detects and registers the vllm binary on Linux
- [ ] `koji backend install vllm` on Windows gives a clear error
- [ ] If vllm is not on PATH, a clear error with install instructions is shown
- [ ] `koji backend list` shows the vLLM backend correctly
- [ ] Config file gets `backend_type = "vllm"` entry with sensible defaults
- [ ] The vLLM flow skips GPU selection and source/prebuilt prompts entirely

---

### Task 4: vLLM quant mapping and configurable health check timeout

**Context:**
vLLM supports quantized model inference via `--quantization` flag (values: `awq`, `gptq`, `fp8`, `bitsandbytes`, etc.). Koji's `ModelConfig.quant` field should map to this when using a vLLM backend. Additionally, vLLM takes 30-120 seconds to load models (much longer than llama.cpp's ~5-10 seconds). The existing `startup_timeout_secs` in `ProxyConfig` (default: 30s) needs to be overridable per-backend.

**Files:**
- Modify: `crates/koji-core/src/config/resolve.rs` — add quant mapping to `build_vllm_args()`
- Modify: `crates/koji-core/src/config/types.rs` — add `startup_timeout_secs` to `BackendConfig`
- Modify: `crates/koji-core/src/proxy/lifecycle.rs` — use per-backend timeout when available
- Modify: `crates/koji-core/src/config/loader.rs` — add default for new field
- Modify: `crates/koji-cli/tests/tests.rs` — add new field to test construction sites
- Test: add tests in `resolve.rs`

**What to implement:**

1. **Quant mapping in `build_vllm_args()`**: After adding `server.args` to the args list, check if `server.quant` is set. If the quant value is one of `["awq", "gptq", "fp8", "bitsandbytes", "marlin", "squeezellm"]` (case-insensitive), add `--quantization <value>` to args (if not already present). If quant contains a `:` character (GGUF-style like `Q4_K_M:filename`), ignore it — it's not applicable to vLLM. If quant is `None` or empty, do nothing.

2. **Add `startup_timeout_secs` to `BackendConfig`** in `types.rs`:
   ```rust
   #[serde(default)]
   pub startup_timeout_secs: Option<u64>,
   ```
   This overrides the global `ProxyConfig.startup_timeout_secs` (default 30s) for this specific backend. **IMPORTANT:** Update all `BackendConfig {` construction sites to add `startup_timeout_secs: None`.

3. **Update `lifecycle.rs`** — search for `let timeout = Duration::from_secs(self.config.proxy.startup_timeout_secs)`. Change this to check the backend's config first:
   ```rust
   let per_backend_timeout = backend_config.startup_timeout_secs;
   let timeout = Duration::from_secs(
       per_backend_timeout.unwrap_or(self.config.proxy.startup_timeout_secs)
   );
   ```

4. **Add tests** in `resolve.rs`:
   - `test_build_vllm_args_quant_awq`: Set `quant = Some("awq")`, verify `--quantization awq` in args.
   - `test_build_vllm_args_quant_gguf_ignored`: Set `quant = Some("Q4_K_M:filename.gguf")`, verify `--quantization` is NOT in args.
   - `test_build_vllm_args_quant_none`: Set `quant = None`, verify `--quantization` is NOT in args.

**Steps:**
- [ ] Add quant mapping logic to `build_vllm_args()` in `resolve.rs`
- [ ] Add `startup_timeout_secs: Option<u64>` to `BackendConfig` in `types.rs`
- [ ] Update ALL `BackendConfig {` construction sites to add `startup_timeout_secs: None`
- [ ] Update `lifecycle.rs` timeout to use per-backend override
- [ ] Add tests for quant mapping
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat: add vLLM quant mapping and per-backend startup timeout`

**Acceptance criteria:**
- [ ] `quant = "awq"` produces `--quantization awq` in vLLM args
- [ ] GGUF-style quant strings (containing `:`) are ignored for vLLM
- [ ] `startup_timeout_secs` in backend config overrides the global proxy timeout
- [ ] Default behavior is unchanged when `startup_timeout_secs` is not set
- [ ] All existing tests still pass

---

### Task 5: Documentation and example config

**Context:**
With vLLM backend support implemented, users need to know how to use it. This task adds an example config block and ensures all help/error text is consistent.

**Files:**
- Modify: `config/koji.toml` — add commented-out vLLM example
- Verify: all error messages across Tasks 1-4 mention vLLM where appropriate

**What to implement:**

1. **Example config**: Add a commented-out vLLM example section at the bottom of `config/koji.toml`:
   ```toml
   # === vLLM Backend (Linux only) ===
   # Install vLLM first: pip install vllm
   # Then register: koji backend install vllm
   #
   # [backends.vllm]
   # path = "/usr/bin/vllm"
   # backend_type = "vllm"
   # default_args = ["--dtype", "auto"]
   # startup_timeout_secs = 120
   #
   # [models.llama8b]
   # backend = "vllm"
   # model = "meta-llama/Llama-3.1-8B-Instruct"
   # context_length = 32768
   # args = ["--trust-remote-code", "--gpu-memory-utilization", "0.85"]
   # profile = "coding"
   # enabled = true
   ```

2. **Verify consistency**: Grep the workspace for any remaining references to "Supported: llama_cpp, ik_llama" that don't include `vllm`. Update them.

**Steps:**
- [ ] Add commented-out vLLM example to `config/koji.toml`
- [ ] Search for stale "Supported:" strings and update to include vllm
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `docs: add vLLM backend example config and update help text`

**Acceptance criteria:**
- [ ] `config/koji.toml` has a clear vLLM example section
- [ ] All "Supported backends" strings include vllm
- [ ] All tests pass
