# Split `platform/windows.rs` into Focused Submodules

**Goal:** Convert the monolithic 458-line `crates/koji-core/src/platform/windows.rs` into a `windows/` directory with focused submodules, without changing any behavior or public API.

**Architecture:** The file contains 11 functions in 4 logical groups: service installation, service lifecycle (start/stop/query/remove), firewall rules, and user permissions. We split into 4 files plus a `mod.rs` that re-exports everything so callers don't change.

**Tech Stack:** Rust modules, `anyhow`, `windows_service` crate

---

## What We're Splitting

Here's every function in `windows.rs` and which file it goes to:

| Function | Visibility | Lines | → New File |
|----------|-----------|-------|------------|
| `wait_for_state` | `fn` (private) | 11–37 | `service.rs` (private helper used by install + lifecycle) |
| `install_service` | `pub fn` | 43–133 | `install.rs` |
| `install_proxy_service` | `pub fn` | 137–213 | `install.rs` |
| `get_current_user_sid` | `fn` (private) | 217–241 | `permissions.rs` |
| `grant_user_control` | `fn` (private) | 246–283 | `permissions.rs` |
| `add_firewall_rule` | `pub fn` | 286–321 | `firewall.rs` |
| `start_service` | `pub fn` | 324–348 | `service.rs` |
| `stop_service` | `pub fn` | 351–373 | `service.rs` |
| `remove_service` | `pub fn` | 376–419 | `service.rs` |
| `remove_firewall_rule` | `pub fn` | 422–435 | `firewall.rs` |
| `query_service` | `pub fn` | 438–458 | `service.rs` |

### New file structure:

```
crates/koji-core/src/platform/
├── mod.rs              ← EXISTING (tiny edit: `pub mod windows` stays as-is)
├── linux.rs            ← UNTOUCHED
├── job_object.rs       ← UNTOUCHED
└── windows/            ← NEW DIRECTORY (replaces windows.rs)
    ├── mod.rs          ← Declares submodules + re-exports all 8 public functions
    ├── install.rs      ← install_service, install_proxy_service
    ├── service.rs      ← wait_for_state, start_service, stop_service, remove_service, query_service
    ├── firewall.rs     ← add_firewall_rule, remove_firewall_rule
    └── permissions.rs  ← get_current_user_sid, grant_user_control
```

### Who calls these functions (DO NOT change these files):

```
crates/koji-cli/src/handlers/server.rs:57    → koji_core::platform::windows::query_service(...)
crates/koji-cli/src/handlers/server.rs:121   → koji_core::platform::windows::query_service(...)
crates/koji-cli/src/handlers/service_cmd.rs:22  → koji_core::platform::windows::install_service(...)
crates/koji-cli/src/handlers/service_cmd.rs:56  → koji_core::platform::windows::install_proxy_service(...)
crates/koji-cli/src/handlers/service_cmd.rs:89  → koji_core::platform::windows::remove_service(...)
crates/koji-cli/src/handlers/service_cmd.rs:110 → koji_core::platform::windows::start_service(...)
crates/koji-cli/src/handlers/service_cmd.rs:128 → koji_core::platform::windows::stop_service(...)
```

**CRITICAL:** All callers use the path `koji_core::platform::windows::function_name`. Because `mod.rs` re-exports everything with `pub use`, callers **do not need to change**. If any caller breaks, the `mod.rs` re-exports are wrong — fix them, don't touch the callers.

---

## Task 1: Create the `windows/` directory and `mod.rs`

### What you're doing
Rust lets you define a module as either a single file (`windows.rs`) or a directory (`windows/mod.rs`). You're converting from the file form to the directory form. They are equivalent — `pub mod windows;` in `platform/mod.rs` works for both.

### Steps

**Step 1.1: Create the directory**

```bash
mkdir crates/koji-core/src/platform/windows
```

**Step 1.2: Create `windows/mod.rs`**

Create the file `crates/koji-core/src/platform/windows/mod.rs` with this EXACT content:

```rust
//! Windows platform support
//!
//! Service installation, lifecycle management, firewall rules, and permissions.

mod firewall;
mod install;
mod permissions;
mod service;

// Re-export all public functions so callers don't need to change.
// e.g. `koji_core::platform::windows::start_service` still works.
pub use firewall::{add_firewall_rule, remove_firewall_rule};
pub use install::{install_proxy_service, install_service};
pub use service::{query_service, remove_service, start_service, stop_service};
```

**Why `mod` not `pub mod`?** The `permissions` and `service` modules contain private helper functions (`wait_for_state`, `get_current_user_sid`, `grant_user_control`). We don't want callers reaching into submodules directly. We use private `mod` declarations and only `pub use` the specific public functions.

**Why no `pub use` for permissions?** `grant_user_control` and `get_current_user_sid` are private functions — they're only called by `install_service` and `install_proxy_service` inside the `install` module. They should NOT be publicly re-exported. The `install` module will use `super::permissions::grant_user_control` to call them.

### Verification

Do NOT try to compile yet. The module files don't exist yet.

---

## Task 2: Create `service.rs`

### What you're doing
Moving the service lifecycle functions into their own file. This file contains the private `wait_for_state` helper (used by install.rs too) and the public `start_service`, `stop_service`, `remove_service`, `query_service` functions.

### Steps

**Step 2.1: Create `windows/service.rs`**

Create the file `crates/koji-core/src/platform/windows/service.rs` with this EXACT content.

Copy these pieces from `windows.rs`:

1. **Imports:** Only the imports this file actually needs. Look at which types each function uses:
   - `wait_for_state` uses: `ServiceState`, `Duration`, `Instant`, `Context`, `Result`
   - `start_service` uses: `ServiceManager`, `ServiceManagerAccess`, `ServiceAccess`, `ServiceState`, `Duration`, `Context`, `Result`
   - `stop_service` uses: same as start_service
   - `remove_service` uses: same + calls `super::firewall::remove_firewall_rule`
   - `query_service` uses: `ServiceManager`, `ServiceManagerAccess`, `ServiceAccess`, `ServiceState`, `Context`, `Result`

```rust
use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use windows_service::service::{ServiceAccess, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
```

2. **`wait_for_state`** — copy lines 9–37 from `windows.rs` VERBATIM. Keep it as `pub(super) fn` (not `pub fn`, not `fn`). This makes it visible to sibling modules (like `install.rs`) but NOT to code outside the `windows/` directory.

Change the signature from:
```rust
fn wait_for_state(
```
to:
```rust
pub(super) fn wait_for_state(
```

3. **`start_service`** — copy lines 323–348 from `windows.rs` VERBATIM. Keep as `pub fn`.

4. **`stop_service`** — copy lines 350–373 from `windows.rs` VERBATIM. Keep as `pub fn`.

5. **`remove_service`** — copy lines 375–419 from `windows.rs` VERBATIM. Keep as `pub fn`. **BUT** change line 416 from:
```rust
    remove_firewall_rule(service_name).ok();
```
to:
```rust
    super::firewall::remove_firewall_rule(service_name).ok();
```
This is because `remove_firewall_rule` is now in a sibling module (`firewall.rs`), not in the same file.

6. **`query_service`** — copy lines 437–458 from `windows.rs` VERBATIM. Keep as `pub fn`.

### Verification

Do NOT try to compile yet. More files are needed.

---

## Task 3: Create `firewall.rs`

### What you're doing
Moving the two firewall functions into their own file.

### Steps

**Step 3.1: Create `windows/firewall.rs`**

Create the file `crates/koji-core/src/platform/windows/firewall.rs` with this EXACT content.

1. **Imports:**

```rust
use anyhow::{Context, Result};
```

That's it. These functions use `std::process::Command` via fully-qualified paths (e.g. `std::process::Command::new(...)`) so no `use` for it is needed.

2. **`add_firewall_rule`** — copy lines 285–321 from `windows.rs` VERBATIM. Keep as `pub fn`.

3. **`remove_firewall_rule`** — copy lines 421–435 from `windows.rs` VERBATIM. Keep as `pub fn`.

### Verification

Do NOT try to compile yet.

---

## Task 4: Create `permissions.rs`

### What you're doing
Moving the two permission functions (SID resolution and SDDL grant) into their own file.

### Steps

**Step 4.1: Create `windows/permissions.rs`**

Create the file `crates/koji-core/src/platform/windows/permissions.rs` with this EXACT content.

1. **Imports:**

```rust
use anyhow::{Context, Result};
```

2. **`get_current_user_sid`** — copy lines 215–241 from `windows.rs` VERBATIM. Keep as `fn` (private). It's only called by `grant_user_control` in this same file, so it doesn't need broader visibility.

3. **`grant_user_control`** — copy lines 243–283 from `windows.rs` VERBATIM. Change visibility from `fn` to `pub(super) fn`. This is called by `install.rs` via `super::permissions::grant_user_control`.

### Verification

Do NOT try to compile yet.

---

## Task 5: Create `install.rs`

### What you're doing
Moving the two install functions. These call into `firewall.rs` and `permissions.rs`, so they need `super::` paths.

### Steps

**Step 5.1: Create `windows/install.rs`**

Create the file `crates/koji-core/src/platform/windows/install.rs` with this EXACT content.

1. **Imports:**

```rust
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::time::{Duration, Instant};
use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
```

2. **`install_service`** — copy lines 39–133 from `windows.rs` VERBATIM. Keep as `pub fn`. Make these changes:

Line 61 — change:
```rust
            wait_for_state(&existing, ServiceState::Stopped, Duration::from_secs(30))
```
to:
```rust
            super::service::wait_for_state(&existing, ServiceState::Stopped, Duration::from_secs(30))
```

Line 125 — change:
```rust
    add_firewall_rule(service_name, port).ok();
```
to:
```rust
    super::firewall::add_firewall_rule(service_name, port).ok();
```

Line 129 — change:
```rust
    grant_user_control(service_name)
```
to:
```rust
    super::permissions::grant_user_control(service_name)
```

3. **`install_proxy_service`** — copy lines 135–213 from `windows.rs` VERBATIM. Keep as `pub fn`. Make these changes:

Line 151 — change:
```rust
            wait_for_state(&existing, ServiceState::Stopped, Duration::from_secs(30))
```
to:
```rust
            super::service::wait_for_state(&existing, ServiceState::Stopped, Duration::from_secs(30))
```

Line 208 — change:
```rust
    add_firewall_rule(service_name, port).ok();
```
to:
```rust
    super::firewall::add_firewall_rule(service_name, port).ok();
```

Line 209 — change:
```rust
    grant_user_control(service_name)
```
to:
```rust
    super::permissions::grant_user_control(service_name)
```

### Verification

Do NOT try to compile yet.

---

## Task 6: Delete the old `windows.rs`

### What you're doing
Now that all the code lives in `windows/mod.rs` + submodules, the old single file must be deleted. Rust will fail to compile if BOTH `windows.rs` and `windows/mod.rs` exist.

### Steps

**Step 6.1: Delete the old file**

```bash
rm crates/koji-core/src/platform/windows.rs
```

### Verification

At this point you should have:
```
crates/koji-core/src/platform/
├── mod.rs              ← unchanged
├── linux.rs            ← unchanged
├── job_object.rs       ← unchanged
└── windows/
    ├── mod.rs
    ├── install.rs
    ├── service.rs
    ├── firewall.rs
    └── permissions.rs
```

The OLD file `crates/koji-core/src/platform/windows.rs` should NOT exist.

---

## Task 7: Verify — build, test, lint

### What you're doing
Making sure nothing is broken.

### Steps

**Step 7.1: Check formatting**

```bash
cargo fmt --all
```

If it reformats anything, that's fine — it means the copy-paste had minor formatting differences. The formatter will fix them.

**Step 7.2: Build the workspace**

```bash
cargo build --workspace
```

**What to do if it fails:**

- `"file not found"` or `"module not found"` → You have a typo in a filename or `mod.rs` declaration. Check spelling of `mod service;` vs `service.rs`, etc.
- `"cannot find function wait_for_state"` → You forgot to change visibility to `pub(super) fn` or forgot the `super::service::` prefix in `install.rs`.
- `"cannot find function grant_user_control"` → Same — check `pub(super) fn` in `permissions.rs` and `super::permissions::` in `install.rs`.
- `"cannot find function add_firewall_rule"` → Check `super::firewall::` prefix in `install.rs` and `service.rs`.
- `"cannot find function remove_firewall_rule"` → Check `super::firewall::` prefix in `service.rs`.
- `"unused import"` → You copied an import that this file doesn't need. Remove it.

**Step 7.3: Run clippy**

```bash
cargo clippy --workspace -- -D warnings
```

**Step 7.4: Run tests**

```bash
cargo test --workspace
```

All 76 tests should pass. None of the windows functions are tested directly (they require a Windows environment), so the tests are just proving we didn't break anything else.

**Step 7.5: Release build**

```bash
cargo build --release --workspace
```

---

## Task 8: Verify no callers changed

### What you're doing
Double-checking that we didn't accidentally modify any file outside the `windows/` directory.

### Steps

**Step 8.1: Check git diff**

```bash
git diff --stat
```

You should see ONLY these changes:
```
 deleted:    crates/koji-core/src/platform/windows.rs
 new file:   crates/koji-core/src/platform/windows/mod.rs
 new file:   crates/koji-core/src/platform/windows/install.rs
 new file:   crates/koji-core/src/platform/windows/service.rs
 new file:   crates/koji-core/src/platform/windows/firewall.rs
 new file:   crates/koji-core/src/platform/windows/permissions.rs
```

**If you see any other files modified** (like `service_cmd.rs`, `server.rs`, `mod.rs`), something went wrong. The whole point is that `mod.rs` re-exports make this transparent to callers.

`platform/mod.rs` should NOT be modified — `pub mod windows;` works for both `windows.rs` and `windows/mod.rs`.

---

## Checklist Before Committing

- [ ] `crates/koji-core/src/platform/windows.rs` is DELETED (does not exist)
- [ ] `crates/koji-core/src/platform/windows/mod.rs` exists and has re-exports
- [ ] `crates/koji-core/src/platform/windows/install.rs` exists
- [ ] `crates/koji-core/src/platform/windows/service.rs` exists
- [ ] `crates/koji-core/src/platform/windows/firewall.rs` exists
- [ ] `crates/koji-core/src/platform/windows/permissions.rs` exists
- [ ] `cargo fmt --all` produces no changes
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` succeeds
- [ ] `cargo test --workspace` — all tests pass
- [ ] `git diff --stat` shows ONLY the 6 files above (1 deleted, 5 new)
- [ ] No other files were modified

**Commit message:** `refactor: split platform/windows.rs into focused submodules`
