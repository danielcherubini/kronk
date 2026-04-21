# Max Loaded Models with LRU Eviction Plan

**Goal:** Add a `max_loaded_models` config field (default=1) that automatically evicts the least-recently-used model when capacity is reached.

**Architecture:** A new `Unloading` state variant prevents lock contention and race conditions during eviction. The eviction method atomically transitions Ready→Unloading under a short-lived write lock, then releases it before calling `unload_model()`. Four auto-load handlers call `evict_lru_if_needed()` before attempting to load.

**Tech Stack:** Rust, Tokio async, Axum, serde (TOML config), rusqlite (unchanged)

---

### Task 1: Config Field — `max_loaded_models` in ProxyConfig

**Context:**
This task introduces the new configuration option that controls the maximum number of simultaneously loaded models. It's a standalone change to the config types and does not depend on any other feature work. The default is 1 (single-model mode), with 0 meaning unlimited/disabled.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs`

**What to implement:**
- Add a private default function `default_max_loaded_models()` returning `u32` value `1`
- Add field `max_loaded_models: u32` to the `ProxyConfig` struct with serde attribute `#[serde(default = "default_max_loaded_models")]` and doc comment explaining: max models, LRU eviction behavior, 0 = unlimited
- Update `impl Default for ProxyConfig` to include `max_loaded_models: default_max_loaded_models()`

**Steps:**
- [ ] Add the private function `fn default_max_loaded_models() -> u32 { 1 }` near the other default functions in `config/types.rs` (around line 270, after `default_download_queue_poll_interval`)
- [ ] Add field to `ProxyConfig` struct:
  ```rust
  /// Maximum number of models that can be loaded simultaneously.
  /// When a new model is requested and the limit is reached, the
  /// least-recently-used (LRU) model is automatically unloaded first.
  /// Set to 0 for unlimited (disabled). Default: 1.
  #[serde(default = "default_max_loaded_models")]
  pub max_loaded_models: u32,
  ```
- [ ] Add `max_loaded_models: default_max_loaded_models(),` to the `impl Default for ProxyConfig` block's `Self { ... }` initializer
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package koji-core`
  - Did it compile without errors? If not, fix any type mismatches or missing imports.
- [ ] Commit with message: `feat(config): add max_loaded_models field to ProxyConfig`

**Acceptance criteria:**
- [ ] `ProxyConfig` has a `max_loaded_models: u32` field with default value 1
- [ ] Setting `max_loaded_models = 0` in config.toml deserializes to 0 (unlimited)
- [ ] Omitting the field uses the default of 1
- [ ] `cargo check --package koji-core` passes cleanly

---

### Task 2: Add `Unloading` State Variant to ModelState

**Context:**
The new `Unloading` state is essential for preventing two problems: (1) holding a write lock on the models map while waiting up to 5 seconds for SIGTERM, which would block all proxy traffic, and (2) race conditions where concurrent eviction attempts try to unload the same model. This task adds the variant and updates every method that pattern-matches on `ModelState`. It's a prerequisite for Task 3 (eviction logic).

**Files:**
- Modify: `crates/koji-core/src/proxy/types.rs`
- Test: `crates/koji-core/src/proxy/lifecycle.rs` (existing test module)

**What to implement:**
- Add `Unloading` variant to the `ModelState` enum with these fields (same as `Ready` minus `load_time`):
  ```rust
  Unloading {
      model_name: String,
      backend: String,
      backend_pid: u32,
      backend_url: String,
      last_accessed: Instant,
      consecutive_failures: Arc<std::sync::atomic::AtomicU32>,
      failure_timestamp: Option<std::time::SystemTime>,
  }
  ```
- Add `#[derive(Default)]` to `ModelState` — default is `Failed { model_name: String::new(), backend: String::new(), error: String::new() }`. This is only used internally as a temporary placeholder during state transitions, so it has no external impact.
- Update ALL existing `ModelState` methods to handle the new variant:
  - `model_name()` → return from Unloading
  - `backend()` → return from Unloading
  - `is_ready()` → return `false` for Unloading (and all non-Ready variants)
  - `backend_url()` → return `None` for Unloading (same as Starting)
  - `backend_pid()` → return `Some(*backend_pid)` from Unloading
  - `consecutive_failures()` → return `Some(consecutive_failures)` from Unloading
  - `load_time()` → return `None` for Unloading (no load_time field)
  - `last_accessed()` → return `Some(last_accessed)` from Unloading
  - `can_reload(cooldown_seconds)` → return `false` for Unloading

**Steps:**
- [ ] Add the `Unloading` variant to the `ModelState` enum in `crates/koji-core/src/proxy/types.rs`, placing it after `Failed` and before the closing brace of the enum
- [ ] Update `model_name()` match: add arm `ModelState::Unloading { model_name, .. } => model_name`
- [ ] Update `backend()` match: add arm `ModelState::Unloading { backend, .. } => backend`
- [ ] Update `is_ready()`: change to return `matches!(self, ModelState::Ready { .. })` (this automatically excludes Unloading)
- [ ] Update `backend_url()`: add arm `ModelState::Unloading { .. } => None`
- [ ] Update `backend_pid()`: add arm `ModelState::Unloading { backend_pid, .. } => Some(*backend_pid)`
- [ ] Update `consecutive_failures()`: add arm for Unloading returning `Some(consecutive_failures)`
- [ ] Update `load_time()`: add arm `ModelState::Unloading { .. } => None`
- [ ] Update `last_accessed()`: add arm `ModelState::Unloading { last_accessed, .. } => Some(*last_accessed)`
- [ ] Update `can_reload(cooldown_seconds)`: add arm for Unloading returning `false`
- [ ] Run `cargo check --package koji-core`
  - Did it compile? If there are other match expressions on ModelState that the compiler flagged (e.g., in lifecycle.rs, status.rs), note them but DO NOT fix them yet — they will be addressed in Task 3. The goal is to get the types module compiling.
- [ ] Commit with message: `feat(proxy): add Unloading state variant to ModelState`

**Acceptance criteria:**
- [ ] `ModelState::Unloading` has all required fields (model_name, backend, backend_pid, backend_url, last_accessed, consecutive_failures, failure_timestamp)
- [ ] All existing `ModelState` methods compile and handle the new variant
- [ ] `is_ready()` returns false for Unloading
- [ ] `last_accessed()` returns Some for Unloading (needed for LRU sorting)
- [ ] `backend_pid()` returns Some for Unloading (needed for SIGTERM target)

---

### Task 3: Eviction Method — `evict_lru_if_needed()`

**Context:**
This is the core logic of the feature. The method checks if the proxy is at capacity, finds the least-recently-used Ready model, atomically transitions it to Unloading (holding the write lock for only microseconds), then releases the lock before calling `unload_model()` (which can take up to 5 seconds). This design prevents both lock contention and race conditions.

**Files:**
- Modify: `crates/koji-core/src/proxy/lifecycle.rs`
- Test: `crates/koji-core/src/proxy/lifecycle.rs` (existing test module at bottom of file)

**What to implement:**
- Add new public async method `evict_lru_if_needed(&self) -> Result<Option<String>>` on `ProxyState`
- Logic:
  1. Read config, get `max_loaded_models`. If 0, return `Ok(None)` (unlimited).
  2. Acquire write lock on `self.models`. If `models.len() < max`, return `Ok(None)`.
  3. Filter to only `ModelState::Ready` variants, find the one with minimum `last_accessed`.
  4. If found, use `std::mem::take()` to atomically transition from Ready → Unloading (copying all fields except `load_time`).
  5. Drop the write lock.
  6. If a model was selected, call `self.unload_model(&name).await` and return `Ok(Some(name))`.
  7. If no Ready model found (all are Starting), return `Ok(None)`.

- Update `unload_model()` to handle the `Unloading` variant:
  - Add a match arm for `ModelState::Unloading { backend_pid, backend, .. }` that performs the same SIGTERM → wait loop → SIGKILL → remove from map flow as the `Ready` arm.
  - The Unloading arm should NOT check `state.is_ready()` — instead check `matches!(state, ModelState::Ready { .. } | ModelState::Unloading { .. })`.
  - After removing from map, update metrics (models_unloaded counter).

- Update `check_idle_timeouts()` to skip `Unloading` models:
  - Add a filter in the iteration: skip models where `matches!(state, ModelState::Unloading { .. })`.

**Steps:**
- [ ] In `crates/koji-core/src/proxy/lifecycle.rs`, add the new method `evict_lru_if_needed`:
  ```rust
  pub async fn evict_lru_if_needed(&self) -> Result<Option<String>> {
      let config = self.config.read().await;
      let max = config.proxy.max_loaded_models;

      // 0 = unlimited (feature disabled)
      if max == 0 {
          return Ok(None);
      }

      let mut models = self.models.write().await;
      if models.len() < max as usize {
          return Ok(None);
      }

      // Find LRU Ready model (skip Starting, Failed, Unloading)
      let lru_name = models.iter()
          .filter(|(_, s)| matches!(s, ModelState::Ready { .. }))
          .min_by_key(|(_, s)| s.last_accessed())
          .map(|(name, _)| name.clone());

      // Atomically transition Ready → Unloading
      if let Some(ref name) = lru_name {
          if let Some(state) = models.get_mut(name) {
              if let ModelState::Ready {
                  model_name, backend, backend_pid, backend_url, last_accessed, consecutive_failures, failure_timestamp,
              } = std::mem::take(state) {
                  *state = ModelState::Unloading {
                      model_name, backend, backend_pid, backend_url, last_accessed, consecutive_failures, failure_timestamp,
                  };
              }
          }
      }

      drop(models); // Release lock BEFORE calling unload_model (can take 5s)

      if let Some(name) = lru_name {
          self.unload_model(&name).await?;
          Ok(Some(name))
      } else {
          // All models are non-Ready (Starting/Failed/Unloading) — can't evict
          Ok(None)
      }
  }
  ```
- [ ] Update `unload_model()`:
  - Change the readiness check from `if !state.is_ready()` to `if !matches!(state, ModelState::Ready { .. } | ModelState::Unloading { .. })`
  - In the match on `state.backend_pid()`, add a case for Unloading: since both Ready and Unloading have `backend_pid()`, the existing `let pid = state.backend_pid()` call will work. Just ensure the match arm that handles `ModelState::Ready` also covers Unloading, or add an explicit arm.
  - Actually, simpler: change the match to use a guard pattern:
    ```rust
    let (backend_name, pid) = match &state {
        ModelState::Ready { backend, backend_pid, .. } | ModelState::Unloading { backend, backend_pid, .. } => {
            (backend.clone(), *backend_pid)
        }
        _ => unreachable!("already checked above"),
    };
    ```
- [ ] Update `check_idle_timeouts()`: in the iteration over `models.iter()`, add a check to skip Unloading models:
  ```rust
  if matches!(state, ModelState::Unloading { .. }) {
      continue;
  }
  ```
- [ ] Add tests in the existing `#[cfg(test)] mod tests` at the bottom of lifecycle.rs:
  - `test_evict_lru_if_needed_zero_is_unlimited`: Create state with default config (max=1), but manually set max_loaded_models to 0. Call evict_lru_if_needed, assert returns `Ok(None)`.
  - `test_evict_lru_if_needed_under_limit_no_eviction`: Create state with max_loaded_models=2, add 1 Ready model. Call evict_lru_if_needed, assert returns `Ok(None)` and model count unchanged.
  - `test_evict_lru_if_needed_at_limit_evicts_lru`: Create state with max_loaded_models=1, add 1 Ready model (set last_accessed to old time). Call evict_lru_if_needed, assert returns `Ok(Some(server_name))` and the model is removed from the map.
  - `test_evict_lru_if_needed_skips_starting_models`: Create state with max_loaded_models=1, add 1 Starting model (not Ready). Call evict_lru_if_needed, assert returns `Ok(None)` and Starting model remains.
  - `test_evict_lru_if_needed_skips_failed_models`: Create state with max_loaded_models=1, add 1 Failed model. Call evict_lru_if_needed, assert returns `Ok(None)`.
  - `test_evict_lru_if_needed_concurrent_no_double_eviction`: Add 2 Ready models with different last_accessed times. Set max_loaded_models=1. Use `tokio::spawn` to run two `evict_lru_if_needed()` calls concurrently. First call should evict the LRU and return `Ok(Some(name))`, second should find no Ready models (the first is now Unloading) and return `Ok(None)`.
- [ ] Run `cargo test --package koji-core -- lifecycle`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package koji-core`
  - Did it compile cleanly? If there are other match expressions on ModelState that the compiler flagged (e.g., in status.rs), note them for a follow-up but DO NOT fix them — they're pre-existing and not part of this feature.
- [ ] Commit with message: `feat(proxy): add LRU eviction when max_loaded_models is reached`

**Acceptance criteria:**
- [ ] `evict_lru_if_needed()` returns `Ok(None)` when max_loaded_models is 0 (unlimited)
- [ ] `evict_lru_if_needed()` returns `Ok(None)` when model count is below the limit
- [ ] `evict_lru_if_needed()` evicts the LRU Ready model when at capacity
- [ ] `evict_lru_if_needed()` skips Starting, Failed, and Unloading models
- [ ] `unload_model()` handles both Ready and Unloading states
- [ ] `check_idle_timeouts()` skips Unloading models (won't try to unload already-unloading models)
- [ ] All 6 new tests pass

---

### Task 4: Handler Integration — Wire Up Eviction in Auto-Load Paths

**Context:**
This final task connects the eviction logic into all four auto-load paths. Every place where the proxy checks if a model is loaded and falls back to `load_model()` now calls `evict_lru_if_needed()` first. This ensures consistent behavior regardless of which API endpoint triggered the load.

**Files:**
- Modify: `crates/koji-core/src/proxy/handlers.rs` (3 locations)
- Modify: `crates/koji-core/src/proxy/koji_handlers/models.rs` (1 location)

**What to implement:**
Insert `let _ = state.evict_lru_if_needed().await;` immediately before the `load_model()` call in each of the four handlers. The eviction result is ignored (`let _`) since:
- If eviction succeeds, the new model loads into a free slot
- If eviction fails (e.g., all models are Starting), load_model proceeds and will fail if no capacity is available — which is correct behavior

**Steps:**
- [ ] In `crates/koji-core/src/proxy/handlers.rs`, in `handle_chat_completions`:
  Find the `None => {` arm inside the `match state.get_available_server_for_model(model_name).await { ... }` block. Insert `let _ = state.evict_lru_if_needed().await;` as the first line of that arm, before `let model_card = ...`.
- [ ] In `crates/koji-core/src/proxy/handlers.rs`, in `handle_stream_chat_completions`:
  Same change — insert `let _ = state.evict_lru_if_needed().await;` as the first line of the `None => {` arm.
- [ ] In `crates/koji-core/src/proxy/handlers.rs`, in `handle_forward_post`:
  Find the `None => {` arm inside the inner `match state.get_available_server_for_model(model).await { ... }`. Insert `let _ = state.evict_lru_if_needed().await;` as the first line.
- [ ] In `crates/koji-core/src/proxy/koji_handlers/models.rs`, in `handle_koji_load_model`:
  Insert `let _ = state.evict_lru_if_needed().await;` as the first line of the function body, after resolving the model_id but before calling `load_model`.
- [ ] Run `cargo test --package koji-core`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
  - Did it compile cleanly? Fix any remaining issues.
- [ ] Commit with message: `feat(proxy): wire up LRU eviction in all auto-load handlers`

**Acceptance criteria:**
- [ ] `handle_chat_completions` calls `evict_lru_if_needed()` before `load_model()`
- [ ] `handle_stream_chat_completions` calls `evict_lru_if_needed()` before `load_model()`
- [ ] `handle_forward_post` calls `evict_lru_if_needed()` before `load_model()`
- [ ] `handle_koji_load_model` calls `evict_lru_if_needed()` before `load_model()`
- [ ] All existing tests still pass (no regressions)
- [ ] `cargo check --workspace` passes cleanly

---

## Implementation Order

1. **Task 1** — Config field (standalone, no dependencies)
2. **Task 2** — Unloading state (prerequisite for Task 3)
3. **Task 3** — Eviction method + tests (depends on Task 2)
4. **Task 4** — Handler wiring (depends on Task 3)

Each task is independently commitable and buildable.

## End-to-End Verification

After all tasks are complete:

1. Set `max_loaded_models = 1` in config.toml
2. Start the proxy, load Model A via `/koji/v1/models/:id/load`
3. Request a different model via `/v1/chat/completions` with `"model": "different-model"`
4. Verify: Model A is unloaded, different-model is loaded (check `/status` or logs)
5. Set `max_loaded_models = 2`, repeat — both models should remain loaded
