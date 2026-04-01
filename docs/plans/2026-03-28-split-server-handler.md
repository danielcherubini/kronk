# Split `handlers/server.rs` into Focused Submodules

**Goal:** Convert the 499-line `crates/kronk-cli/src/handlers/server.rs` into a `server/` directory with one file per command, without changing any behavior or public API.
**Status:** DONE

**Architecture:** The file has 6 functions: a dispatcher, 4 public command handlers (ls/add/edit/rm), and 1 private helper (`resolve_backend`). We split into 5 files + a `mod.rs`. The dispatcher and shared helper live in `mod.rs`, each command gets its own file.

---

## What We're Splitting

Here's every function in `server.rs` and which file it goes to:

| Function | Visibility | Lines | Line Count | → New File |
|----------|-----------|-------|------------|------------|
| `cmd_server` | `pub async fn` | 9–26 | 18 | `mod.rs` (dispatcher) |
| `resolve_backend` | `fn` (private) | 178–240 | 63 | `mod.rs` (shared helper, made `pub(super)`) |
| `cmd_server_ls` | `pub async fn` | 29–108 | 80 | `ls.rs` |
| `cmd_server_add` | `pub async fn` | 243–427 | 185 | `add.rs` |
| `cmd_server_edit` | `pub async fn` | 430–499 | 70 | `edit.rs` |
| `cmd_server_rm` | `pub fn` | 111–162 | 52 | `rm.rs` |

### New file structure:

```
crates/kronk-cli/src/handlers/
├── mod.rs              ← EXISTING (tiny edit: `pub mod server` stays as-is)
├── server/             ← NEW DIRECTORY (replaces server.rs)
│   ├── mod.rs          ← Dispatcher + resolve_backend + re-exports
│   ├── ls.rs           ← cmd_server_ls
│   ├── add.rs          ← cmd_server_add
│   ├── edit.rs         ← cmd_server_edit
│   └── rm.rs           ← cmd_server_rm
├── config.rs           ← UNTOUCHED
├── logs.rs             ← UNTOUCHED
├── profile.rs          ← UNTOUCHED
├── run.rs              ← UNTOUCHED
├── serve.rs            ← UNTOUCHED
├── service_cmd.rs      ← UNTOUCHED
└── status.rs           ← UNTOUCHED
```

### Who references these functions (DO NOT change these files):

```
crates/kronk-cli/src/lib.rs:19   → pub use handlers::server::{cmd_server_add, cmd_server_edit};
crates/kronk-cli/src/lib.rs:24   → use handlers::...server...
crates/kronk-cli/src/lib.rs:64   → server::cmd_server_add(&config, &name, command, false).await
crates/kronk-cli/src/lib.rs:67   → server::cmd_server_edit(&mut config.clone(), &name, command).await
crates/kronk-cli/src/lib.rs:69   → server::cmd_server(&config, command).await
crates/kronk-cli/tests/tests.rs:178 → use kronk::{cmd_server_add, cmd_server_edit};
```

**CRITICAL:** All callers use the path `handlers::server::function_name` or the re-export from `lib.rs`. Because `mod.rs` re-exports everything with `pub use`, callers **do not need to change**. If any caller breaks, the `mod.rs` re-exports are wrong — fix them, don't touch the callers.

---

## Task 1: Create the `server/` directory and `mod.rs`

### What you're doing
Converting `server.rs` (single file) → `server/mod.rs` (directory module). You're keeping the dispatcher function and the shared `resolve_backend` helper here, and re-exporting the command functions from submodules.

### Steps

**Step 1.1: Create the directory**

```bash
mkdir crates/kronk-cli/src/handlers/server
```

**Step 1.2: Create `server/mod.rs`**

Create the file `crates/kronk-cli/src/handlers/server/mod.rs` with this content:

```rust
//! Server command handler
//!
//! Handles `kronk server ls/add/edit/rm` commands.

mod add;
mod edit;
mod ls;
mod rm;

// Re-export all public command functions so callers don't change.
// e.g. `handlers::server::cmd_server_add` still works.
pub use add::cmd_server_add;
pub use edit::cmd_server_edit;
pub use ls::cmd_server_ls;
pub use rm::cmd_server_rm;

use anyhow::{Context, Result};
use kronk_core::config::Config;

/// Manage servers — list, add, edit, remove
pub async fn cmd_server(config: &Config, command: crate::cli::ServerCommands) -> Result<()> {
    match command {
        crate::cli::ServerCommands::Ls => cmd_server_ls(config).await,
        crate::cli::ServerCommands::Add { name, command } => {
            cmd_server_add(config, &name, command, false).await
        }
        crate::cli::ServerCommands::Edit { name, command } => {
            if !config.models.contains_key(&name) {
                anyhow::bail!(
                    "Server '{}' not found. Use `kronk server add` to create it.",
                    name
                );
            }
            cmd_server_edit(&mut config.clone(), &name, command).await
        }
        crate::cli::ServerCommands::Rm { name, force } => cmd_server_rm(config, &name, force),
    }
}
```

Then, below that, paste the `resolve_backend` function — copy lines 164–240 from the original `server.rs` **VERBATIM**. But change the visibility from `fn` to `pub(super) fn`:

Change:
```rust
fn resolve_backend(config: &mut Config, exe_path: &str) -> Result<(String, String)> {
```
to:
```rust
pub(super) fn resolve_backend(config: &mut Config, exe_path: &str) -> Result<(String, String)> {
```

**Why `pub(super)`?** `resolve_backend` is called by both `add.rs` and `edit.rs`. Those are sibling submodules — they can access `pub(super)` items from their parent (`mod.rs`). We don't want it fully `pub` because it's an implementation detail, not a public API.

**Why does `resolve_backend` live in `mod.rs` and not its own file?** It's a shared helper used by exactly 2 siblings. Putting it in mod.rs makes it available to all children via `super::resolve_backend`. If it were in its own file (e.g., `util.rs`), both `add.rs` and `edit.rs` would need `super::util::resolve_backend` — more indirection for no benefit.

### Verification

Do NOT try to compile yet. The submodule files don't exist yet.

---

## Task 2: Create `ls.rs`

### What you're doing
Moving the `cmd_server_ls` function (lines 28–108, 80 lines) into its own file. This function lists all configured servers with their status and health.

### Steps

**Step 2.1: Create `server/ls.rs`**

Create the file `crates/kronk-cli/src/handlers/server/ls.rs`.

1. **Imports:** Look at what `cmd_server_ls` actually uses:
   - `anyhow::Result` — for the return type
   - `kronk_core::config::Config` — for the function parameter

```rust
use anyhow::Result;
use kronk_core::config::Config;
```

That's it. The function uses `reqwest::Client`, `kronk_core::platform::windows::query_service`, `kronk_core::platform::linux::query_service`, and `Config::service_name` — but all via fully-qualified paths already in the source code, so they don't need `use` imports.

2. **`cmd_server_ls`** — copy lines 28–108 from `server.rs` VERBATIM. Keep as `pub async fn`. No changes needed — this function doesn't call `resolve_backend` or any other function in `server.rs`.

### Verification

Do NOT try to compile yet.

---

## Task 3: Create `rm.rs`

### What you're doing
Moving the `cmd_server_rm` function (lines 110–162, 52 lines) into its own file. This function removes a server from config after checking for installed services.

### Steps

**Step 3.1: Create `server/rm.rs`**

Create the file `crates/kronk-cli/src/handlers/server/rm.rs`.

1. **Imports:**

```rust
use anyhow::{Context, Result};
use kronk_core::config::Config;
```

`Context` is needed because line 149 uses `.context("Confirmation cancelled")`.

2. **`cmd_server_rm`** — copy lines 110–162 from `server.rs` VERBATIM. Keep as `pub fn` (note: this is a sync function, NOT async). No changes needed — this function doesn't call `resolve_backend` or any other function in `server.rs`.

### Verification

Do NOT try to compile yet.

---

## Task 4: Create `add.rs`

### What you're doing
Moving the `cmd_server_add` function (lines 242–427, 185 lines) into its own file. This is the biggest and most complex function — it resolves backends, looks up model cards, validates quants, and builds a `ModelConfig`.

### Steps

**Step 4.1: Create `server/add.rs`**

Create the file `crates/kronk-cli/src/handlers/server/add.rs`.

1. **Imports:**

```rust
use anyhow::{Context, Result};
use kronk_core::config::Config;
```

That's it. The function uses `kronk_core::models::ModelRegistry`, `kronk_core::profiles::Profile`, `kronk_core::config::ModelConfig`, and `inquire::Select` — but all via fully-qualified paths in the source. The `use anyhow::Context;` on line 249 of the original is a local import inside the function body — we move it to the file-level import instead, so **delete the `use anyhow::Context;` line from inside the function body** (line 249 in the original).

2. **`cmd_server_add`** — copy lines 242–427 from `server.rs`. Keep as `pub async fn`. Make these changes:

**Change 1:** Delete the inner `use anyhow::Context;` on the first line of the function body (line 249 in original). We already have it in the file-level imports.

The original looks like:
```rust
pub async fn cmd_server_add(
    config: &Config,
    name: &str,
    command: Vec<String>,
    overwrite: bool,
) -> Result<()> {
    use anyhow::Context;

    if command.is_empty() {
```

Change it to:
```rust
pub async fn cmd_server_add(
    config: &Config,
    name: &str,
    command: Vec<String>,
    overwrite: bool,
) -> Result<()> {
    if command.is_empty() {
```

**Change 2:** Line 261 calls `resolve_backend`. Since that function now lives in the parent `mod.rs`, change:
```rust
    let (backend_key, exe_str) = resolve_backend(&mut config, exe_path)?;
```
to:
```rust
    let (backend_key, exe_str) = super::resolve_backend(&mut config, exe_path)?;
```

**Change 3:** Line 264 calls `crate::flags::extract_kronk_flags`. This does NOT need to change — it's already a fully-qualified path from the crate root.

**That's it.** Only 2 changes: remove inner `use` statement, add `super::` to `resolve_backend`.

### Verification

Do NOT try to compile yet.

---

## Task 5: Create `edit.rs`

### What you're doing
Moving the `cmd_server_edit` function (lines 429–499, 70 lines) into its own file.

### Steps

**Step 5.1: Create `server/edit.rs`**

Create the file `crates/kronk-cli/src/handlers/server/edit.rs`.

1. **Imports:**

```rust
use anyhow::Result;
use kronk_core::config::Config;
```

No `Context` needed — `cmd_server_edit` doesn't use `.context()` or `.with_context()`.

2. **`cmd_server_edit`** — copy lines 429–499 from `server.rs`. Keep as `pub async fn`. Make this one change:

**Change 1:** Line 443 calls `resolve_backend`. Change:
```rust
    let (backend_key, exe_str) = resolve_backend(config, exe_path)?;
```
to:
```rust
    let (backend_key, exe_str) = super::resolve_backend(config, exe_path)?;
```

**That's it.** Only 1 change: add `super::` to `resolve_backend`.

### Verification

Do NOT try to compile yet.

---

## Task 6: Delete the old `server.rs`

### What you're doing
Now that all the code lives in `server/mod.rs` + submodules, the old single file must be deleted.

### Steps

**Step 6.1: Delete the old file**

```bash
rm crates/kronk-cli/src/handlers/server.rs
```

### Verification

At this point you should have:
```
crates/kronk-cli/src/handlers/
├── mod.rs              ← UNCHANGED
├── server/             ← NEW DIRECTORY
│   ├── mod.rs          ← Dispatcher + resolve_backend + re-exports
│   ├── ls.rs           ← cmd_server_ls
│   ├── add.rs          ← cmd_server_add
│   ├── edit.rs         ← cmd_server_edit
│   └── rm.rs           ← cmd_server_rm
├── config.rs           ← UNCHANGED
├── logs.rs             ← UNCHANGED
├── profile.rs          ← UNCHANGED
├── run.rs              ← UNCHANGED
├── serve.rs            ← UNCHANGED
├── service_cmd.rs      ← UNCHANGED
└── status.rs           ← UNCHANGED
```

The OLD file `crates/kronk-cli/src/handlers/server.rs` should NOT exist.

---

## Task 7: Verify — build, test, lint

### Steps

**Step 7.1: Check formatting**

```bash
cargo fmt --all
```

If it reformats anything, that's fine.

**Step 7.2: Build the workspace**

```bash
cargo build --workspace
```

**What to do if it fails:**

- `"cannot find function resolve_backend"` in `add.rs` or `edit.rs` → You forgot the `super::` prefix. Change `resolve_backend(...)` to `super::resolve_backend(...)`.
- `"cannot find function cmd_server_ls"` (or add/edit/rm) in `mod.rs` → The `pub use` re-export is wrong. Check spelling: `pub use ls::cmd_server_ls;` etc.
- `"unused import"` for `Context` → A file imports `Context` but doesn't use it. Remove it from that file's imports.
- `"module server not found"` → The directory name is wrong or `mod.rs` is missing. Check that `crates/kronk-cli/src/handlers/server/mod.rs` exists (not `crates/kronk-cli/src/handlers/server.rs`).
- `"duplicate module"` → You have BOTH `server.rs` and `server/mod.rs`. Delete `server.rs`.

**Step 7.3: Run clippy**

```bash
cargo clippy --workspace -- -D warnings
```

**Step 7.4: Run tests**

```bash
cargo test --workspace
```

All 76 tests should pass. The 3 server tests (`test_cmd_server_add_nonexistent_model_errors`, `test_cmd_server_edit_nonexistent_server_errors`, `test_cmd_server_edit_valid_profile_succeeds`) exercise `cmd_server_add` and `cmd_server_edit` through the re-exports in `lib.rs`. If they fail, the re-export chain is broken — check `server/mod.rs` re-exports.

**Step 7.5: Release build**

```bash
cargo build --release --workspace
```

---

## Task 8: Verify no callers changed

### Steps

**Step 8.1: Check git diff**

```bash
git diff --stat
```

You should see ONLY these changes:
```
 deleted:    crates/kronk-cli/src/handlers/server.rs
 new file:   crates/kronk-cli/src/handlers/server/mod.rs
 new file:   crates/kronk-cli/src/handlers/server/ls.rs
 new file:   crates/kronk-cli/src/handlers/server/add.rs
 new file:   crates/kronk-cli/src/handlers/server/edit.rs
 new file:   crates/kronk-cli/src/handlers/server/rm.rs
```

**If you see any other files modified** (like `lib.rs`, `handlers/mod.rs`, `tests.rs`), something went wrong. The whole point is that `mod.rs` re-exports make this transparent to callers.

`handlers/mod.rs` should NOT be modified — `pub mod server;` works for both `server.rs` and `server/mod.rs`.

---

## Checklist Before Committing

- [ ] `crates/kronk-cli/src/handlers/server.rs` is DELETED (does not exist)
- [ ] `crates/kronk-cli/src/handlers/server/mod.rs` exists with dispatcher + resolve_backend + re-exports
- [ ] `crates/kronk-cli/src/handlers/server/ls.rs` exists with `cmd_server_ls`
- [ ] `crates/kronk-cli/src/handlers/server/add.rs` exists with `cmd_server_add`
- [ ] `crates/kronk-cli/src/handlers/server/edit.rs` exists with `cmd_server_edit`
- [ ] `crates/kronk-cli/src/handlers/server/rm.rs` exists with `cmd_server_rm`
- [ ] `add.rs` does NOT have `use anyhow::Context;` inside the function body (moved to file-level)
- [ ] `add.rs` calls `super::resolve_backend(...)`, NOT `resolve_backend(...)`
- [ ] `edit.rs` calls `super::resolve_backend(...)`, NOT `resolve_backend(...)`
- [ ] `cargo fmt --all` produces no changes
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` succeeds
- [ ] `cargo test --workspace` — all 76 tests pass (including the 3 server tests)
- [ ] `git diff --stat` shows ONLY the 6 files above (1 deleted, 5 new)
- [ ] No other files were modified (`lib.rs`, `handlers/mod.rs`, `tests.rs` are all UNCHANGED)

**Commit message:** `refactor: split handlers/server.rs into focused submodules`
