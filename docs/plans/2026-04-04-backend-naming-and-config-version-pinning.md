# Backend Naming and Config Version Pinning Plan

**Goal:** Backends install with simple names (`llama_cpp`, `ik_llama`); the DB stores all version history; `config.toml` optionally pins a specific version hash — the DB is the store, config.toml is the master.

**Architecture:**
- `koji backend install llama_cpp` always installs under the canonical name `llama_cpp` (not `llama_cpp_b8407`). The DB tracks the version hash internally.
- `config.toml [backends.llama_cpp]` gains an optional `version` field. When set, `resolve_backend_path` looks up the DB for that specific version of that backend. When absent, the active (latest) version is used.
- The `path` field in `BackendConfig` is kept for custom/manual installs (unchanged behaviour).

**Tech Stack:** Rust, SQLite (rusqlite), TOML (serde)

---

### Task 1: Remove versioned name generation in `cmd_install`; fix same-version reinstall

**Context:**
Currently in `crates/koji-cli/src/commands/backend.rs`, when no `--name` is supplied, the install command generates a versioned name like `llama_cpp_b8407` (lines 298–305). This makes it awkward to reference backends by name in `config.toml` because the name changes with every install. The fix is to default the name to the canonical backend type string (`llama_cpp` or `ik_llama`) so the name is stable across updates.

Additionally: because both old and new installs now go to `backends/llama_cpp/`, reinstalling the same version would hit the `UNIQUE(name, version)` DB constraint and fail. Fix `insert_backend_installation` in `crates/koji-core/src/db/queries.rs` to use `INSERT OR REPLACE` so that reinstalling the same `(name, version)` is idempotent (it just updates the row in-place). This also means `cmd_update` naturally replaces the old path when it installs to the same directory.

**Files:**
- Modify: `crates/koji-cli/src/commands/backend.rs`
- Modify: `crates/koji-core/src/db/queries.rs`

**What to implement:**

*In `backend.rs`*, change the `backend_name` fallback (lines 298–305) so that when `name` is `None`, the name is just the canonical type string. Extract it to a testable helper:

```rust
fn default_backend_name(bt: &BackendType) -> String {
    match bt {
        BackendType::LlamaCpp => "llama_cpp".to_string(),
        BackendType::IkLlama => "ik_llama".to_string(),
        BackendType::Custom => "custom".to_string(),
    }
}
```

Then in `cmd_install`:
```rust
let backend_name = name.unwrap_or_else(|| default_backend_name(&backend_type));
```

The `--name` flag still allows overrides for power users who want multiple installs side-by-side (e.g., a CPU and a CUDA build).

The install directory `target_dir = backends_dir()?.join(&backend_name)` stays the same — default installs go to `~/.config/koji/backends/llama_cpp/`.

*In `queries.rs`*, change `insert_backend_installation` from `INSERT INTO` to `INSERT OR REPLACE INTO` so reinstalling the same `(name, version)` is idempotent. The `UPDATE ... SET is_active = 0 WHERE name = ?1 AND version != ?2` step (which deactivates old versions) stays unchanged.

**Steps:**
- [ ] Add a test `test_insert_duplicate_version_is_idempotent` in `queries.rs` tests: insert `("llama_cpp", "b8407")` twice and assert no error and only one row exists. (The existing `test_insert_duplicate_version_fails` test will need to be updated or removed since the behaviour is changing — update it to assert idempotency instead.)
- [ ] Run `cargo test --package koji-core -- test_insert_duplicate`
  - Expected: existing test passes (INSERT fails), new test does not exist yet.
- [ ] Change `INSERT INTO` to `INSERT OR REPLACE INTO` in `insert_backend_installation`.
- [ ] Update the existing `test_insert_duplicate_version_fails` test to `test_insert_same_version_is_idempotent` asserting no error and count = 1.
- [ ] Run `cargo test --package koji-core -- test_insert`
  - All insert tests pass?
- [ ] Add `fn default_backend_name(bt: &BackendType) -> String` to `backend.rs` with a `#[cfg(test)]` unit test asserting it returns `"llama_cpp"`, `"ik_llama"`, `"custom"` for the three variants.
- [ ] Update `cmd_install` to call `default_backend_name`.
- [ ] Run `cargo test --package koji-cli`
  - All tests pass?
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit: `fix: default backend install name to canonical type; make same-version reinstall idempotent`

**Acceptance criteria:**
- [ ] `koji backend install llama_cpp` without `--name` registers backend as `"llama_cpp"`
- [ ] `koji backend install llama_cpp --name my_llama` still names it `"my_llama"`
- [ ] Reinstalling the same `(name, version)` does not error
- [ ] Build and clippy clean

---

### Task 2: Add `version` field to `BackendConfig` + `get_backend_by_version` query + wire up `resolve_backend_path`

**Context:**
`BackendConfig` in `crates/koji-core/src/config/types.rs` currently has `path`, `default_args`, and `health_check_url`. We add an optional `version` field so users can pin a specific version hash in `config.toml`:

```toml
[backends.llama_cpp]
version = "b4567"
```

When `version` is set, `resolve_backend_path` looks for that exact `(name, version)` in the DB. When absent, it uses the active (latest) version. Because the branching logic and the DB query are tightly coupled, both are implemented in this single task to avoid a half-working intermediate state.

Note: `BackendConfig` does not derive `Default`, so every manual construction site in tests (e.g. `make_test_config` in `resolve.rs` tests and `tests/tests.rs`) must add `version: None` when constructing `BackendConfig` literals. These will be compile errors so they are easy to catch.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs`
- Modify: `crates/koji-core/src/db/queries.rs`
- Modify: `crates/koji-core/src/config/resolve.rs`

**What to implement:**

*In `types.rs`*, add to `BackendConfig`:
```rust
/// Optional version pin. When set, resolve_backend_path looks up this
/// specific version in the DB instead of the currently-active version.
#[serde(default)]
pub version: Option<String>,
```

*In `queries.rs`*, add:
```rust
/// Get a specific backend installation by (name, version).
/// Returns Ok(None) if no row matches.
pub fn get_backend_by_version(
    conn: &Connection,
    name: &str,
    version: &str,
) -> Result<Option<BackendInstallationRecord>> {
    // SELECT ... FROM backend_installations WHERE name = ?1 AND version = ?2
}
```

*In `resolve.rs`*, update `resolve_backend_path`. Change the priority from:
1. Active DB record → 2. config path

To:
1. If `config.backends[name].version` is `Some(v)` → call `get_backend_by_version(conn, name, v)`. If found return its path. If not found, return a descriptive error: `"Backend 'llama_cpp' version 'b4567' not found in DB. Run \`koji backend install llama_cpp\` first."`
2. Otherwise → `get_active_backend(conn, name)` (existing behaviour)
3. Fallback to `config.backends[name].path` (existing behaviour)

**Steps:**
- [ ] Write a test `test_get_backend_by_version` in `queries.rs` `#[cfg(test)]` module: insert two version rows for `llama_cpp`, assert the correct path is returned for each version string, and `Ok(None)` for an unknown version.
- [ ] Run `cargo test --package koji-core -- test_get_backend_by_version`
  - Fails (function doesn't exist)? Good.
- [ ] Implement `get_backend_by_version` in `queries.rs`.
- [ ] Run `cargo test --package koji-core -- test_get_backend_by_version`
  - Passes? Good.
- [ ] Add `version: Option<String>` to `BackendConfig` in `types.rs`.
- [ ] Fix all compile errors caused by `BackendConfig` struct literals missing `version: None` (check `resolve.rs` tests and `crates/koji-cli/tests/tests.rs`).
- [ ] Write a test `test_resolve_backend_path_version_pin` in `resolve.rs` tests: insert two versions of `llama_cpp` into an in-memory DB; set `config.backends["llama_cpp"].version = Some("v1.0.0")`; assert `resolve_backend_path` returns the v1 path (not v2 active).
- [ ] Write a test `test_resolve_backend_path_version_pin_not_found` in `resolve.rs` tests: set `version = Some("nonexistent")`; assert `resolve_backend_path` returns `Err` with a message containing `"not found in DB"`.
- [ ] Run `cargo test --package koji-core -- test_resolve_backend_path_version_pin`
  - Fails (branching logic not yet updated)? Good.
- [ ] Update `resolve_backend_path` in `resolve.rs` with the branching logic.
- [ ] Run `cargo test --package koji-core`
  - All tests pass? If not, fix.
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit: `feat: add version pin to BackendConfig and resolve_backend_path`

**Acceptance criteria:**
- [ ] `BackendConfig` has `version: Option<String>` serialised/deserialised correctly with TOML round-trip
- [ ] `get_backend_by_version("llama_cpp", "b4567")` returns the correct record; unknown version returns `Ok(None)`
- [ ] `resolve_backend_path` with `version = Some("b4567")` returns that version's path
- [ ] `resolve_backend_path` with pinned version not in DB returns a descriptive error
- [ ] All existing `resolve_backend_path` tests still pass
- [ ] Full workspace build and clippy clean

---

### Task 3: Update `koji backend list` to show version info clearly

**Context:**
Now that backends install under canonical names but track versions in the DB, the `list` output should show the installed version hash clearly so users know what they have and can reference the version string in `config.toml`. Currently `cmd_list` shows `name (BackendType)`, version, and path. Enhance it to be cleaner and add a hint about version pinning.

Also, `cmd_install`'s success message currently says:
```
  koji server add my-server <binary_path> ...
```
Update it to use the canonical name instead of the binary path, and add a note showing the version that was installed (so users know what to put in `config.toml` if they want to pin it).

**Files:**
- Modify: `crates/koji-cli/src/commands/backend.rs`

**What to implement:**

In `cmd_list`, change output to:
```
Installed backends:

  llama_cpp (llama.cpp)
    Version:  b8407
    Path:     /home/user/.config/koji/backends/llama_cpp/llama-server
    GPU:      Cuda { version: "12.4" }

  ik_llama (ik_llama.cpp)
    Version:  main@abc1234  (or the git hash / "main" for source builds)
    Path:     ...
```

In `cmd_install`, after success, update the hint:
```
Installation complete!
  Name:    llama_cpp
  Version: b8407
  Binary:  /home/user/.config/koji/backends/llama_cpp/llama-server

To use this backend, it is already referenced in config.toml as 'llama_cpp'.
To pin this exact version in config.toml:
  [backends.llama_cpp]
  version = "b8407"
```

No new logic is needed — just string formatting changes in `cmd_install` and `cmd_list`.

**Steps:**
- [ ] Update `cmd_install` success message to include the version and pin hint.
- [ ] Update `cmd_list` formatting.
- [ ] Run `cargo build --package koji-cli`
  - Build succeeds? If not, fix.
- [ ] Run `cargo fmt --all && cargo clippy --package koji-cli -- -D warnings`
- [ ] Commit: `chore: improve backend list and install output with version pin hints`

**Acceptance criteria:**
- [ ] `koji backend install` output shows the version and how to pin it
- [ ] `koji backend list` shows name, version, path, GPU clearly
- [ ] Build and clippy clean

---

### Task 4: Update `Config::default()` and `config.toml` template

**Context:**
`Config::default()` in `crates/koji-core/src/config/loader.rs` pre-populates `backends` with `llama_cpp` and `ik_llama` entries that have `path: None`. With the new `version` field, these should still default to `None` for both `path` and `version` (so the active DB installation is used). The `config/koji.toml` template should document the new optional `version` field with a comment so users know it exists.

No behaviour changes needed — just documentation/defaults hygiene.

**Files:**
- Modify: `crates/koji-core/src/config/loader.rs`
- Modify: `config/koji.toml`

**What to implement:**

In `loader.rs` `Default for Config`, the `BackendConfig` struct now has a `version` field — ensure it's set to `None` in both default entries (the derive `Default` already handles this if `#[serde(default)]` is set, but be explicit for clarity).

In `config/koji.toml`, add commented-out examples:
```toml
[backends.llama_cpp]
# version = "b8407"   # pin a specific version; omit to use the latest installed
# path = "/custom/path/llama-server"   # or point directly to a binary

[backends.ik_llama]
# version = "main@abc1234"
```

**Steps:**
- [ ] Update `config/koji.toml` with comments.
- [ ] Verify `loader.rs` default `BackendConfig` instances compile with the new `version` field (add `version: None` explicitly).
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
  - All tests pass? If not, fix.
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit: `docs: document version pin field in config.toml template and defaults`

**Acceptance criteria:**
- [ ] `config/koji.toml` template has comments explaining `version` and `path` fields
- [ ] `Config::default()` compiles and all tests pass
- [ ] Full workspace build and clippy clean
