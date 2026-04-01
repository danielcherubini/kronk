# kronk-cli Code Quality Improvements Plan

**Goal:** Improve code quality, consistency, and maintainability of the kronk-cli crate through targeted refactoring, dead code removal, and documentation.
**Status:** DONE

**Architecture:** Five focused tasks addressing real issues found via code review. Each task is independently committable and ordered by impact.

**Tech Stack:** Rust, anyhow, tokio, clap, tracing

---

### Task 1: Remove Dead Code Duplication in cli.rs

`cli.rs` contains a full copy of `ExtractedFlags` (lines 12-25) and `extract_kronk_flags()` (lines 48-168) that duplicates `flags.rs`. The `flags.rs` version is the canonical one — it's re-exported from `lib.rs` and used by `handlers/server.rs`. The `cli.rs` copy is dead code.

**Files:**
- Modify: `crates/kronk-cli/src/cli.rs` — remove `ExtractedFlags` struct (lines 12-25) and `extract_kronk_flags()` function (lines 48-168)

**Steps:**
- [ ] Verify no code imports from `cli::ExtractedFlags` or `cli::extract_kronk_flags` (grep the crate)
- [ ] Remove `ExtractedFlags` struct and `extract_kronk_flags` fn from `cli.rs`
- [ ] Remove the `use anyhow::Context;` import in `cli.rs` if it becomes unused
- [ ] Run `cargo test --package kronk` — all existing tests use `flags::extract_kronk_flags`
- [ ] Run `cargo clippy --package kronk -- -D warnings`
- [ ] Commit: `chore: remove dead ExtractedFlags and extract_kronk_flags duplicate from cli.rs`

---

### Task 2: Fix stderr/stdout Misuse for Error Messages

Several `println!` calls output error/warning messages to stdout instead of stderr. This breaks pipe/redirection workflows.

**Specific changes:**
- `commands/backend.rs:514` — `println!("Skipping file removal: {}", e)` → `eprintln!`
- `commands/backend.rs:530` — `println!("Skipping file removal: {}", e)` → `eprintln!`
- `commands/backend.rs:539` — `println!("Skipping file removal: path is outside managed directory.")` → `eprintln!`
- `commands/backend.rs:542` — `println!("Skipping file removal: directory does not exist.")` → `eprintln!`
- `commands/backend.rs:578` — `println!("error: {}", e)` → `eprintln!`

**Files:**
- Modify: `crates/kronk-cli/src/commands/backend.rs`

**Steps:**
- [ ] Change the 5 identified `println!` calls to `eprintln!`
- [ ] Run `cargo build --package kronk`
- [ ] Run `cargo clippy --package kronk -- -D warnings`
- [ ] Commit: `fix: use stderr for error/warning messages in backend commands`

---

### Task 3: Remove Duplicate `build_full_args` Wrapper Functions

Three identical one-liner wrapper functions exist that all just call `config.build_full_args()`:
- `handlers/run.rs:72-78`
- `handlers/service_cmd.rs:143-149`
- `service.rs:359-365` (behind `#[cfg(target_os = "windows")]`)

The wrappers add no value — callers should call `config.build_full_args()` directly.

**Files:**
- Modify: `crates/kronk-cli/src/handlers/run.rs` — inline the call at line 13, remove fn at lines 72-78
- Modify: `crates/kronk-cli/src/handlers/service_cmd.rs` — inline the call at line 33, remove fn at lines 143-149
- Modify: `crates/kronk-cli/src/service.rs` — inline the call at line 278, remove fn at lines 359-365

**Steps:**
- [ ] In `handlers/run.rs`: replace `build_full_args(config, server, backend, ctx_override)?` with `config.build_full_args(server, backend, ctx_override)?`, remove the private `build_full_args` fn
- [ ] In `handlers/service_cmd.rs`: replace `build_full_args(config, srv, backend, None)?` with `config.build_full_args(srv, backend, None)?`, remove the private `build_full_args` fn
- [ ] In `service.rs`: replace `build_full_args(&config, srv, backend, ctx)` with `config.build_full_args(srv, backend, ctx)`, remove the `#[cfg(target_os = "windows")]` `build_full_args` fn
- [ ] Run `cargo build --package kronk`
- [ ] Run `cargo test --package kronk`
- [ ] Run `cargo clippy --package kronk -- -D warnings`
- [ ] Commit: `refactor: remove redundant build_full_args wrapper functions`

---

### Task 4: Add Security Documentation to Path Validation

The `cmd_remove` function in `backend.rs` has important security checks (canonical path validation to prevent directory traversal) but no comments explaining why. Future contributors could accidentally weaken these checks.

**Files:**
- Modify: `crates/kronk-cli/src/commands/backend.rs` — add doc comments around lines 488-494

**Steps:**
- [ ] Add a `// SECURITY:` comment block above the canonicalization logic (line ~488) explaining:
  - Why both paths are canonicalized (prevents symlink and `..` traversal attacks)
  - Why `starts_with` check is needed (ensures deletion stays within managed `backends/` dir)
  - What happens if canonicalization fails (deletion is skipped — safe default)
- [ ] Run `cargo build --package kronk`
- [ ] Commit: `docs: document security rationale for path validation in backend removal`

---

### Task 5: Add `args.rs` Dead Code Audit

`args.rs` contains `inject_context_size()` which is marked `#[cfg(test)]` and `#[allow(dead_code)]` — it's only compiled in test builds and even then unused. This is leftover code.

**Files:**
- Modify: `crates/kronk-cli/src/args.rs` — either remove the function or add a test that uses it
- Modify: `crates/kronk-cli/src/lib.rs` — remove `pub mod args;` if the module becomes empty

**Steps:**
- [ ] Check if `inject_context_size` is used anywhere (grep the workspace)
- [ ] If unused: remove the function and the `args` module declaration from `lib.rs`
- [ ] If used in tests: remove the `#[allow(dead_code)]` attribute
- [ ] Run `cargo test --package kronk`
- [ ] Run `cargo clippy --package kronk -- -D warnings`
- [ ] Commit: `chore: remove unused inject_context_size from args.rs`

---

### Verification Steps

After completing all tasks:
1. `cargo build --workspace`
2. `cargo test --workspace`
3. `cargo clippy --workspace -- -D warnings`
4. `cargo fmt --all -- --check`

---

### Tasks Considered and Rejected

| Proposal | Reason for rejection |
|----------|---------------------|
| Create `config_loader.rs` module | CLI and Windows service are separate execution paths — they *should* load config independently. Adding abstraction adds complexity without benefit. |
| Create `utils.rs` for path caching | `Config::base_dir()` is cheap (env var lookup + path join, no I/O). `registry_path()` and `backends_dir()` are called once per command invocation. No performance issue exists. |
| Add integration tests for new refactors | The refactors are mechanical (dead code removal, inlining). Existing tests already cover the affected code paths. New tests would test the same thing. |
| Add documentation to all public APIs | Most handlers already have adequate doc comments. A blanket documentation pass risks producing boilerplate docs that add noise without value. |

---

**Estimated time:** 30-45 minutes
**Risk:** Very low — tasks 1, 2, 3, 5 are mechanical removals/renames; task 4 is comments-only
