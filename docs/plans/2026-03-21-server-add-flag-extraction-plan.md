# Server Add/Edit Flag Extraction - Implementation Plan

**Goal:** Make `server add` and `server edit` extract koji-specific flags (`--model`, `--profile`, `--quant`, `--port`, `--ctx`) from raw args into proper `ModelConfig` struct fields, with interactive quant selection and model card validation.
**Architecture:** Add an `extract_koji_flags()` helper that parses the raw arg vec, separating koji flags from backend passthrough args. Refactor `cmd_server_add` and `cmd_server_edit` to use it. Extract shared backend resolution logic into `resolve_backend()`.
**Tech Stack:** Rust, clap (existing), inquire (existing), koji-core ModelRegistry

**Status:** ✅ COMPLETED - See git commit `c8327c8` ("Feature/server flag extraction (#12)") and `4de3b5a` ("feat: unified status command, remove model ps, fix logs_dir path mismatch")

---

### Task 1: Add `extract_koji_flags()` helper with tests

**Files:**
- Modify: `crates/koji-cli/src/main.rs`
- Modify: `crates/koji-cli/tests/tests.rs`

**Steps:**
- [ ] Write tests for `extract_koji_flags()` in `tests.rs`:
  - `--model unsloth/Qwen3.5-0.8B` extracted as model card ref
  - `--model /path/to/file.gguf` left in remaining args
  - `-m /path/to/file.gguf` left in remaining args (short flag)
  - `--profile chat` extracted
  - `--quant Q4_K_M` extracted
  - `--port 8081` extracted as `u16`
  - `--ctx 8192` extracted as `u32`
  - Mixed koji and backend flags separated correctly
  - No koji flags → all args remain as backend args
  - `--model` without a value → returns error
  - `--model repo/name --quant Q8_0 --host 0.0.0.0 -t 8` → model+quant extracted, host+threads remain
  - `--quant Q4_K_M` without `--model` → quant extracted, no error (warning handled at call site)
- [ ] Run tests, verify they fail
- [ ] Implement `ExtractedFlags` struct and `extract_koji_flags()` function in `main.rs` with doc comments:
  ```rust
  struct ExtractedFlags {
      model: Option<String>,
      quant: Option<String>,
      profile: Option<String>,
      port: Option<u16>,
      context_length: Option<u32>,
      remaining_args: Vec<String>,
  }

  fn extract_koji_flags(args: Vec<String>) -> Result<ExtractedFlags>
  ```
  - Iterate args, detect `--model`, `--profile`, `--quant`, `--port`, `--ctx`
  - For `--model`: check if value looks like model card ref (contains `/`, no `.gguf`, not absolute path) — extract; otherwise leave as backend arg
  - For all extracted flags: consume the flag AND its value from remaining args
  - Return error if a flag has no following value
- [ ] Run tests, verify they pass
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit

### Task 2: Add `resolve_backend()` shared helper

**Files:**
- Modify: `crates/koji-cli/src/main.rs`

**Steps:**
- [ ] Extract the backend path resolution logic from `cmd_server_add` (lines 1221-1280) into a new function with doc comments:
  ```rust
  fn resolve_backend(
      config: &mut Config,
      exe_path: &str,
  ) -> Result<String>  // returns backend_key
  ```
  - Handles path absolutization (filesystem path vs bare command)
  - Finds existing backend by path or creates a new one
  - Returns the backend key
- [ ] Run `cargo build --workspace` to verify compilation
- [ ] Run `cargo test --workspace`
- [ ] Commit

### Task 3: Update `cmd_server_add()` to use flag extraction

**Files:**
- Modify: `crates/koji-cli/src/main.rs`

**Steps:**
- [ ] Write an integration-style test for `cmd_server_add` with a temp config dir (if feasible) that verifies:
  - `--model repo/name --profile chat` results in `ModelConfig.model`, `.profile` being set and `args` not containing them
  - Raw args like `--host 0.0.0.0` remain in `ModelConfig.args`
  - `--model nonexistent/model` errors with "not found" message
  - `--quant InvalidQuant` with valid `--model` errors with available quants listed
- [ ] Refactor `cmd_server_add` to:
  1. Split `command[0]` (exe) from `command[1..]` (args)
  2. Call `resolve_backend()` for backend resolution
  3. Call `extract_koji_flags()` on the args
  4. If `extracted.model` is `Some`:
     - Look up model card via `ModelRegistry::find()`
     - If not found, error with helpful message
     - If `extracted.quant` is `None`, run quant selection (interactive picker if multiple, auto-select if one)
     - If `extracted.quant` is `Some`, validate it exists in model card
     - Verify GGUF file exists on disk
  5. Parse `extracted.profile` via `Profile::from_str` if present
  6. Build `ModelConfig` with extracted fields set, `remaining_args` as `args`
  7. Set `source` to model ID when model card ref is extracted
  8. Update output to show model/quant/profile/GGUF path when available
- [ ] Run tests, verify they pass
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit

### Task 4: Update `cmd_server_edit()` to use flag extraction with field preservation

**Files:**
- Modify: `crates/koji-cli/src/main.rs`

**Steps:**
- [ ] Write a test for `cmd_server_edit` that verifies:
  - Editing with `--profile coding` preserves existing `model` and `quant` fields
  - Editing with `--model` + `--quant` overwrites those fields
  - Editing with only backend args preserves all koji fields
  - Editing with `--quant` when existing `model` is set validates against existing model card
  - Editing with `--model nonexistent/model` errors appropriately
- [ ] Refactor `cmd_server_edit` to:
  1. Load the existing `ModelConfig` for the server
  2. Split command, call `resolve_backend()` and `extract_koji_flags()`
  3. Selectively merge extracted flags into existing `ModelConfig`:
     - Only overwrite `model` if `extracted.model` is `Some`
     - Only overwrite `quant` if `extracted.quant` is `Some`
     - Only overwrite `profile` if `extracted.profile` is `Some`
     - Only overwrite `port` if `extracted.port` is `Some`
     - Only overwrite `context_length` if `extracted.context_length` is `Some`
     - Always update `backend` and `args` (backend from new exe, args from `remaining_args`)
  4. If model card ref is provided, validate it (same as server add)
  5. If quant is provided without model, and existing model is set, validate against existing model card
  6. Update `source` when model changes
  7. Update output messages
- [ ] Run tests, verify they pass
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit

### Task 5: Verify and clean up

**Files:**
- All modified files

**Steps:**
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Manual verification: run `koji server add test_server llama-server --model unsloth/Qwen3.5-0.8B --profile chat` and inspect config.toml output
- [ ] Verify no unused imports
- [ ] Commit if cleanup needed
