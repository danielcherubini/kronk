# Remove llama.cpp Hardcoded Defaults Plan

**Goal:** Remove hardcoded `llama_cpp` and `ik_llama` backend entries from the default config and template, making tama truly backend-agnostic from first boot.

**Architecture:** Backend data (path, default_args, health_check_url) lives in SQLite via `BackendManager`. The TOML `[backends]` section is optional — `EMPTY_BACKEND_CONFIG` fallback handles missing entries. Two defensive fixes are needed first: `status.rs` skips models when backend is missing from TOML, and `resolve/mod.rs` has overly strict guards that block health URL resolution.

**Tech Stack:** Rust, Cargo

**Task ordering:** Task 1 MUST complete before Task 3. If Task 3 (removing defaults) is applied first, existing tests will fail because `build_status_response` still skips models with `None => continue`. Tasks 1 and 2 are independent of each other.

---

### Task 1: Fix `status.rs` — don't skip models when backend missing from TOML

**Context:**
`build_status_response()` in `proxy/status.rs` (~line 86) does `config.backends.get(&model_config.backend)` and skips the model entirely (`continue`) when the backend is not in TOML. This means after removing hardcoded defaults, ALL models would disappear from the status endpoint. The `backend_path` field is just displayed in the JSON — it should fall back to `null`, not skip the model.

**Files:**
- Modify: `crates/tama-core/src/proxy/status.rs`

**What to implement:**
- In `build_status_response()` (~line 86), change:
  ```rust
  // BEFORE
  let backend_path = match config.backends.get(&model_config.backend) {
      Some(b) => b.path.clone(),
      None => continue,
  };
  ```
  to:
  ```rust
  // AFTER
  let backend_path = config.backends
      .get(&model_config.backend)
      .and_then(|b| b.path.clone());
  ```
- The `backend_path` variable flows into `serde_json::json!` macros. `Option<String>` serializes correctly: `Some(path)` → JSON string, `None` → JSON `null`.

**Steps:**
- [ ] Write a test in `proxy/mod.rs` tests module: `test_build_status_response_backend_path_null`
  - Create a `Config::default()` (which will have empty backends after Task 3, but for this test explicitly clear backends: `config.backends.clear()`)
  - Add a model with `backend: "llama_cpp"` to `model_configs`
  - Call `build_status_response()` and verify the model appears in the response with `backend_path: null`
- [ ] Run `cargo test --package tama-core proxy::tests::test_build_status_response_backend_path_null -- --nocapture`
  - Did it fail? If the current code skips the model, the test should fail because the model won't appear in the response.
- [ ] Implement the fix in `crates/tama-core/src/proxy/status.rs`
- [ ] Run `cargo test --package tama-core proxy::tests::test_build_status_response_backend_path_null -- --nocapture`
  - Did the test pass?
- [ ] Run `cargo test --package tama-core proxy::tests` to verify no regressions
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "fix: don't skip models from status when backend missing from TOML"

**⚠️ Do NOT proceed to Task 3 until this task's tests pass.** Task 3 depends on this fix.

**Acceptance criteria:**
- [ ] Models appear in status response even when their backend is not in TOML config
- [ ] `backend_path` is `null` in JSON when backend path is not available
- [ ] All existing proxy tests pass

---

### Task 2: Fix `resolve/mod.rs` — remove overly strict backend guards

**Context:**
`resolve_health_url` (~line 114) and `resolve_backend_url` (~line 154) in `config/resolve/mod.rs` have guards that `return None` when the backend is not in TOML config. The comments say "all callers go through resolve_server first" — but `resolve_server` already uses `EMPTY_BACKEND_CONFIG` fallback. These guards are overly strict and would block health URL resolution for all models when backends HashMap is empty. The health URL comes from the DB (`BackendManager.get_health_check_url`), not the TOML.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs`

**What to implement:**
- In `resolve_health_url` (~line 114-126), remove the backend-existence guard block:
  ```rust
  // REMOVE THIS BLOCK
  let _backend = match self.backends.get(&server.backend) {
      Some(b) => b,
      None => {
          tracing::warn!(...);
          return None;
      }
  };
  ```
  The function already handles the `health_check_url` parameter provided by callers. The TOML backend entry is irrelevant.

- In `resolve_backend_url` (~line 154-166), remove the same guard block.

- Update the doc comments to remove the claim that backend must exist in TOML.

- Add a `tracing::debug!` in `resolve_server` and `resolve_servers_for_model` when the `EMPTY_BACKEND_CONFIG` fallback is used, to preserve debugging visibility:
  ```rust
  // In resolve_server, replace:
  let backend = self.backends
      .get(&server.backend)
      .unwrap_or(&EMPTY_BACKEND_CONFIG);
  // With:
  let backend = match self.backends.get(&server.backend) {
      Some(b) => b,
      None => {
          tracing::debug!(
              "Backend '{}' not in TOML [backends] section; using DB-backed defaults",
              server.backend
          );
          &EMPTY_BACKEND_CONFIG
      }
  };
  ```
  Apply the same pattern in `resolve_servers_for_model` (~line 78-80).

- What NOT to change: The `EMPTY_BACKEND_CONFIG` static itself — just add the debug log around its use.

**Steps:**
- [ ] Write a test in `config/resolve/tests/server_resolution.rs`: `test_resolve_health_url_without_toml_backend`
  - Construct a `Config` with `backends = HashMap::new()` (no entries)
  - Create a `ModelConfig` with `backend: "llama_cpp"`, `port: Some(8080)`
  - Call `resolve_health_url(&model, Some("http://localhost:9090/health"))` and assert `Some("http://localhost:9090/health")` (parameter takes priority)
  - Call `resolve_health_url(&model, None)` and assert `Some("http://localhost:8080/health")` (port fallback, not blocked by missing TOML entry)
  - Do the same for `resolve_backend_url` — check it returns a base URL derived from port when TOML entry is absent
- [ ] Run `cargo test --package tama-core config::resolve::tests::server_resolution::test_resolve_health_url_without_toml_backend -- --nocapture`
  - Did it fail? The current guard should return `None`.
- [ ] Implement the fix in `crates/tama-core/src/config/resolve/mod.rs`
- [ ] Run `cargo test --package tama-core config::resolve::tests::server_resolution::test_resolve_health_url_without_toml_backend -- --nocapture`
  - Did the test pass?
- [ ] Run `cargo test --package tama-core config::resolve` to verify no regressions
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "fix: remove overly strict backend guards from resolve_health_url and resolve_backend_url"

**Acceptance criteria:**
- [ ] `resolve_health_url` returns the health URL even when backend is not in TOML
- [ ] `resolve_backend_url` returns the backend URL even when backend is not in TOML
- [ ] All existing resolve tests pass

---

### Task 3: Remove hardcoded backend defaults + add kronk TODO

**Context:**
Now that the defensive fixes are in place, it's safe to remove the hardcoded `llama_cpp` and `ik_llama` entries from `Config::default()` and the config template. Also add a removal TODO to the kronk migration code.

**Files:**
- Modify: `crates/tama-core/src/config/loader.rs`
- Modify: `config/tama.toml`
- Modify: `crates/tama-core/src/config/rename_legacy.rs`

**What to implement:**

1. In `loader.rs`, `Config::default()`:
   - Remove the two `backends.insert(...)` calls for `llama_cpp` and `ik_llama`
   - Use `backends: HashMap::new()` instead

2. In `config/tama.toml`:
   - Remove the `[backends.llama_cpp]` section (lines with `# version`, `# path`, `# default_args`, `health_check_url`)
   - Remove the `[backends.ik_llama]` section
   - The file should only have: `[general]`, `[models]`, `[supervisor]`, `[proxy]`

3. In `rename_legacy.rs`, at the top of the file (before the module doc comment):
   ```rust
   // TODO(v1.60): Remove this entire module. See loader.rs migration call site.
   // Once removed, also remove the `rename_legacy` module declaration from config/mod.rs
   // and the migration call in Config::base_dir().
   ```

4. In `loader.rs`, on the line calling `migrate_legacy_data_dir(&base)`:
   ```rust
   // TODO(v1.60): Remove kronk→tama migration. By v1.60 all users will have
   // migrated or started fresh. Remove this call and the rename_legacy module.
   if let Err(e) = super::rename_legacy::migrate_legacy_data_dir(&base) {
   ```

**Steps:**
- [ ] In `crates/tama-core/src/config/loader.rs`, replace the backends initialization in `Config::default()`:
  ```rust
  // REMOVE
  let mut backends = HashMap::new();
  backends.insert("llama_cpp".to_string(), BackendConfig { ... });
  backends.insert("ik_llama".to_string(), BackendConfig { ... });
  ```
  with:
  ```rust
  // ADD
  let backends = HashMap::new();
  ```
- [ ] In `config/tama.toml`, remove the `[backends.llama_cpp]` and `[backends.ik_llama]` sections
- [ ] In `crates/tama-core/src/config/rename_legacy.rs`, add the TODO comment at top of file
- [ ] In `crates/tama-core/src/config/loader.rs`, add the TODO comment on the migration call
- [ ] Run `cargo test --package tama-core` to verify ALL tests pass with empty backends
  - Pay special attention to: `proxy::tests`, `config::resolve::tests`, `config::tests`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "refactor: remove hardcoded llama_cpp/ik_llama backend defaults from config"

**Acceptance criteria:**
- [ ] `Config::default()` has an empty `backends` HashMap
- [ ] `config/tama.toml` has no `[backends.*]` sections
- [ ] All `tama-core` tests pass (`cargo test --package tama-core`)
- [ ] `cargo clippy --package tama-core -- -D warnings` is clean
- [ ] kronk migration code has TODO(v1.60) with clear instructions
- [ ] `resolve_server` emits `tracing::debug!` when falling back to `EMPTY_BACKEND_CONFIG`

**Note on `health_check_url`:** The `config/tama.toml` currently has `health_check_url = "http://localhost:8080/health"` for both backends. After removal, health URLs come from `BackendManager.get_health_check_url()` which reads from the `backend_configs` SQLite table. This is populated when backends are installed via `tama backend install`. For fresh installs, the health URL is set during the install process. Existing users who have already installed backends have the data in their DB. Users who only have the TOML entry and no DB record will get `None` from `BackendManager`, which is handled by the existing fallback logic (port-based URL construction).
