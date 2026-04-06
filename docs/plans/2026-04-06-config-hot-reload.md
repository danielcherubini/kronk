# Config Hot-Reload Plan

**Goal:** When config or models are saved via the web UI, the in-memory config used by the proxy should be updated immediately — no restart required.

**Architecture:** The web UI server (`kronk-web`) has its own `AppState` that is completely disconnected from the proxy's `ProxyState`. When saving, it reads config from disk, modifies it, and writes back to disk — but never updates the `ProxyState.config` `Arc<RwLock<Config>>` that the proxy actually uses for routing, model loading, and backend resolution. The fix is to share the `ProxyState.config` Arc with `AppState` so that writes through the web API also update the live in-memory config.

**Tech Stack:** Rust, Axum, Tokio, TOML, `Arc<RwLock<Config>>`

---

## Root Cause

There are two independent state holders:

1. **`ProxyState.config`** (`Arc<tokio::sync::RwLock<Config>>`) — the live config in `kronk-core`. Created once at startup in `serve.rs` and used by all proxy handlers. Only mutated by `setup_model_after_pull()` after a model download.

2. **`AppState`** (`kronk-web`) — holds only a `config_path: Option<PathBuf>`. Every web API endpoint (`save_config`, `update_model`, `create_model`, `rename_model`, `delete_model`) calls `load_config_from_state()` which reads config fresh from **disk**, mutates the in-memory copy, writes back to **disk**, and returns. The proxy's `Arc<RwLock<Config>>` is never touched.

Result: config changes are persisted to disk but the running proxy keeps using the stale config from startup.

---

### Task 1: Add proxy config handle to AppState

**Context:**
The `AppState` struct in `kronk-web/src/server.rs` currently has no connection to `ProxyState`. We need to add an optional `Arc<RwLock<Config>>` field so web API handlers can update the live config after saving to disk. It's `Option` because the web server can run standalone (without a proxy).

**Files:**
- Modify: `crates/kronk-web/src/server.rs`
- Test: `crates/kronk-web/tests/server_test.rs`

**What to implement:**
- Add a new field to `AppState`:
  ```rust
  pub proxy_config: Option<Arc<tokio::sync::RwLock<kronk_core::config::Config>>>,
  ```
- Update `run_with_opts()` to accept an additional `proxy_config: Option<Arc<tokio::sync::RwLock<kronk_core::config::Config>>>` parameter and pass it into the `AppState` constructor.
- Update `run()` (convenience wrapper) to pass `None` for the new parameter.
- Update the web UI spawn site in `crates/kronk-cli/src/handlers/serve.rs` to pass `Some(Arc::clone(&state.config))` so the web server shares the proxy's config Arc.
- Do NOT change any API handler logic yet — that's Task 2.

**Steps:**
- [ ] Add the `proxy_config` field to `AppState` in `crates/kronk-web/src/server.rs`
- [ ] Update `run_with_opts()` signature and body to accept and pass through `proxy_config`
- [ ] Update `run()` to pass `None`
- [ ] Update `crates/kronk-cli/src/handlers/serve.rs` to pass `Some(Arc::clone(&state.config))` when spawning the web UI
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: "feat: plumb proxy config Arc into web AppState"

**Acceptance criteria:**
- [ ] `AppState` has a `proxy_config` field of type `Option<Arc<tokio::sync::RwLock<kronk_core::config::Config>>>`
- [ ] When the web UI is spawned from `kronk serve`, it receives the proxy's config Arc
- [ ] All existing tests pass
- [ ] No API behavior change yet

---

### Task 2: Reload in-memory config after web API saves

**Context:**
With the proxy config Arc now available in `AppState`, we need to update each web API handler that saves config to also reload the in-memory config. There are two categories of save operations:

1. **Raw config editor** (`save_config` in `api.rs`): writes raw TOML to disk. After writing, it should parse the TOML into a `Config` struct and replace the proxy's in-memory config.

2. **Model CRUD** (`create_model`, `update_model`, `delete_model`, `rename_model` in `api.rs`): these load config from disk, modify it, save it back to disk. After saving, they should also write the updated config into the proxy's Arc.

**Files:**
- Modify: `crates/kronk-web/src/api.rs`
- Test: `crates/kronk-web/tests/server_test.rs`

**What to implement:**

Add a helper function in `api.rs`:
```rust
/// Update the proxy's live in-memory config after a successful disk save.
/// No-op if proxy_config is None (standalone web server without proxy).
async fn sync_proxy_config(
    state: &AppState,
    new_config: kronk_core::config::Config,
) {
    if let Some(ref proxy_config) = state.proxy_config {
        let mut config = proxy_config.write().await;
        *config = new_config;
    }
}
```

Then update each handler:

**`save_config`:** After successfully writing TOML to disk, parse the validated TOML into a `Config` (already done for validation — but currently the parsed result is discarded; restructure to keep it), **restore `loaded_from`** from the existing proxy config (since `loaded_from` is `#[serde(skip)]`, `toml::from_str` always produces `None` — without restoring it, `models_dir()`, `configs_dir()`, and `save()` will all fail), and call `sync_proxy_config`. Note: `save_config` runs the file write inside `spawn_blocking`, so the `sync_proxy_config` call should happen outside the blocking closure, after the file write succeeds.

```rust
// Critical: restore loaded_from after parsing raw TOML
let mut new_config: Config = toml::from_str(&body.content)?;
if let Some(ref proxy_config) = state.proxy_config {
    let existing = proxy_config.read().await;
    new_config.loaded_from = existing.loaded_from.clone();
}
sync_proxy_config(&state, new_config).await;
```

**`update_model`:** After `cfg.save_to(&config_dir)` succeeds, clone the `cfg` and return it from the `spawn_blocking` closure. Then call `sync_proxy_config` with the cloned config.

**`create_model`:** Same pattern as `update_model`.

**`delete_model`:** Same pattern as `update_model`.

**`rename_model`:** Same pattern as `update_model`.

Important: `sync_proxy_config` calls `.write().await` on the tokio `RwLock`, which must NOT be called inside `spawn_blocking` (it requires an async context). Structure the code so the blocking file I/O happens in `spawn_blocking`, and the async config sync happens after the join.

**Steps:**
- [ ] Write a test in `crates/kronk-web/tests/server_test.rs` that:
  1. Creates a temp config directory with a valid config
  2. Creates an `AppState` with `proxy_config = Some(Arc::new(RwLock::new(config)))`
  3. Calls the model create or update endpoint
  4. Asserts that the `proxy_config` Arc now contains the updated model
  - This test should fail initially because the sync logic doesn't exist yet.
- [ ] Run `cargo test --package kronk-web`
  - Did it fail with the expected assertion error? If it passed, stop and investigate.
- [ ] Add the `sync_proxy_config` helper to `crates/kronk-web/src/api.rs`
- [ ] Update `save_config` to call `sync_proxy_config` after successful file write
- [ ] Update `update_model` to return the updated `Config` from `spawn_blocking` and call `sync_proxy_config`
- [ ] Update `create_model` to return the updated `Config` from `spawn_blocking` and call `sync_proxy_config`
- [ ] Update `delete_model` to return the updated `Config` from `spawn_blocking` and call `sync_proxy_config`
- [ ] Update `rename_model` to return the updated `Config` from `spawn_blocking` and call `sync_proxy_config`
- [ ] Run `cargo test --package kronk-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: "feat: sync in-memory proxy config after web UI saves"

**Acceptance criteria:**
- [ ] After saving config via `POST /api/config`, the proxy's in-memory config is updated
- [ ] After creating/updating/deleting/renaming a model via the model API, the proxy's in-memory config is updated
- [ ] New model entries are immediately visible in `GET /v1/models` without restart
- [ ] Config changes (e.g. backend paths) take effect without restart
- [ ] `loaded_from` is always preserved when syncing config to the proxy (test this explicitly)
- [ ] All existing tests pass
- [ ] No deadlocks or panics — async sync is done outside `spawn_blocking`

**Known limitations (document, do not fix in this PR):**
- `reqwest::Client` timeout is baked at startup from `idle_timeout_secs + 30` — changing `idle_timeout_secs` via the web UI won't affect the HTTP client timeout (but the idle unload check reads from the config Arc and will work correctly)
- Already-running models continue with their original settings (backend, port, sampling); config changes only take effect on next model load
- TOCTOU race: if a pull job completes (`setup_model_after_pull`) while a web CRUD handler is mid-flight, one can overwrite the other's changes. Mitigate in a follow-up by reading from the proxy config Arc instead of disk in the CRUD handlers.

---

### Task 3: Add integration test for config hot-reload

**Context:**
We need to verify end-to-end that saving a model through the web API makes it visible through the proxy's model listing, without restarting. This test should simulate the full flow using the actual router.

**Files:**
- Modify: `crates/kronk-web/tests/server_test.rs`

**What to implement:**
Write an integration test that:
1. Creates a `ProxyState` with a default config
2. Creates an `AppState` that shares the `ProxyState.config` Arc
3. Builds both routers (or just the web router)
4. Sends a `POST /api/models` request to create a new model
5. Reads the proxy's `config.read().await.models` and asserts the new model is present
6. Sends a `PUT /api/models/:id` request to update the model
7. Reads the proxy config again and asserts the update is reflected
8. Sends a `DELETE /api/models/:id` request
9. Reads the proxy config and asserts the model is gone
10. Sends a `POST /api/config` with modified TOML and asserts the proxy config is updated

**Steps:**
- [ ] Write the integration test in `crates/kronk-web/tests/server_test.rs`
- [ ] Run `cargo test --package kronk-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: "test: add integration test for config hot-reload via web API"

**Acceptance criteria:**
- [ ] Integration test verifies full CRUD cycle with config synchronization
- [ ] Test uses the actual Axum router (not just unit-testing functions)
- [ ] Test confirms proxy config Arc is updated after each web API mutation
- [ ] All tests pass
