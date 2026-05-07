# Process Health Monitor Plan

**Goal:** Detect dead backend processes after Proxmox LXC suspend/resume and auto-restart them; catch models stuck in "Starting" state.

**Architecture:** Extend the existing `check_idle_timeouts()` periodic loop (runs every 30s or `idle_timeout_secs / 2`) to verify PID liveness for Ready models and timeout for Starting models. Add `start_time: Instant` to `ModelState::Starting` and `restart_count: u32` to `ModelState::Ready` / `ModelState::Unloading`. Auto-restart is spawned (not awaited) to keep the health check tick fast.

**Tech Stack:** Rust, tokio, existing `is_process_alive()` primitive, existing `load_model()` lifecycle.

---

### Task 1: Add new fields to ModelState

**Context:**
The `ModelState` enum needs two new fields to support the health monitor:
- `start_time: Instant` on `Starting` — records when loading began, so the periodic check can detect stuck Starting states (using `last_accessed` is incorrect because it gets updated on incoming requests during startup, which would push the timeout indefinitely)
- `restart_count: u32` on `Ready` and `Unloading` — tracks how many times the health monitor has auto-restarted this model after detecting a dead PID. When this reaches `supervisor.max_restarts`, the model transitions to Failed instead of restarting again.

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs`

**What to implement:**

In `crates/tama-core/src/proxy/types.rs`, modify the `ModelState` enum:

1. Add `start_time: Instant` field to `ModelState::Starting` variant. Place it after `last_accessed`.

2. Add `restart_count: u32` field to `ModelState::Ready` variant. Place it after `failure_timestamp`.

3. Add `restart_count: u32` field to `ModelState::Unloading` variant. Place it after `failure_timestamp`.

4. Update `ModelState::default()` — no change needed (Failed variant is unchanged).

5. Add a new helper method on `ModelState`:
   ```rust
   /// Get the restart count for this model (only set on Ready/Unloading states).
   pub fn restart_count(&self) -> Option<u32> {
       match self {
           ModelState::Ready { restart_count, .. } => Some(*restart_count),
           ModelState::Unloading { restart_count, .. } => Some(*restart_count),
           _ => None,
       }
   }
   ```

6. Add a new helper method on `ModelState`:
   ```rust
   /// Get the start time for Starting state models.
   pub fn start_time(&self) -> Option<Instant> {
       match self {
           ModelState::Starting { start_time, .. } => Some(*start_time),
           _ => None,
       }
   }
   ```

**Steps:**
- [ ] Add `start_time: Instant` to `ModelState::Starting` in `types.rs`
- [ ] Add `restart_count: u32` to `ModelState::Ready` in `types.rs`
- [ ] Add `restart_count: u32` to `ModelState::Unloading` in `types.rs`
- [ ] Add `restart_count()` helper method to `impl ModelState`
- [ ] Add `start_time()` helper method to `impl ModelState`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
  - This will fail because all construction sites need the new fields. Expected compile errors in:
    - `lifecycle.rs` — `load_model()`, `load_tts_backend()`, `evict_lru_if_needed()`, test helpers
    - `server/mod.rs` — `cleanup_stale_processes()` constructs `ModelState::Ready`
    - `mod.rs` — test `test_proxy_state_shutdown_clears_models()` constructs `ModelState::Ready`
    - `status.rs` — test constructs `ModelState::Ready`
  - **Do NOT fix these yet** — they are the expected compile errors that Tasks 2 and 3 will fix. Just verify the errors are only about missing fields.
- [ ] Commit with message: "feat: add start_time and restart_count fields to ModelState"

**Acceptance criteria:**
- [ ] `ModelState::Starting` has `start_time: Instant` field
- [ ] `ModelState::Ready` has `restart_count: u32` field
- [ ] `ModelState::Unloading` has `restart_count: u32` field
- [ ] `restart_count()` and `start_time()` helper methods compile and work
- [ ] Compile errors are ONLY about missing `start_time` and `restart_count` fields in construction sites

---

### Task 2: Update all construction sites for new fields

**Context:**
After Task 1 adds the new fields, every place that constructs a `ModelState` variant must provide the new fields. This task fixes ALL construction sites across the codebase. `restart_count: 0` is used everywhere except the auto-restart path (Task 3), because a fresh load/reconnect always starts at 0 restarts.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle.rs`
- Modify: `crates/tama-core/src/proxy/server/mod.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs` (test code)
- Modify: `crates/tama-core/src/proxy/status.rs` (test code)

**What to implement:**

1. In `lifecycle.rs`, `load_model()` — `ModelState::Starting` insertion: add `start_time: Instant::now()`

2. In `lifecycle.rs`, `load_model()` — `ModelState::Ready` construction: add `restart_count: 0`

3. In `lifecycle.rs`, `evict_lru_if_needed()` — capture `restart_count` from Ready, preserve in Unloading:
   ```rust
   if let ModelState::Ready {
       model_name, backend, backend_pid, backend_url,
       last_accessed, consecutive_failures, failure_timestamp,
       restart_count,  // NEW
   } = std::mem::take(state)
   {
       *state = ModelState::Unloading {
           model_name, backend, backend_pid, backend_url,
           last_accessed, consecutive_failures, failure_timestamp,
           restart_count,  // NEW — preserve
       };
   }
   ```

4. In `lifecycle.rs`, `load_tts_backend()` — `ModelState::Starting` insertion: add `start_time: Instant::now()`

5. In `lifecycle.rs`, `load_tts_backend()` — `ModelState::Ready` construction: add `restart_count: 0`

6. In `server/mod.rs`, `cleanup_stale_processes()` — `ModelState::Ready` construction: add `restart_count: 0` (this is a reconnect after startup, so 0 restarts)

7. In `mod.rs` test `test_proxy_state_shutdown_clears_models()` — add `restart_count: 0`

8. In `status.rs` test — add `restart_count: 0`

**Steps:**
- [ ] Fix `load_model()` Starting state — add `start_time: Instant::now()`
- [ ] Fix `load_model()` Ready state — add `restart_count: 0`
- [ ] Fix `evict_lru_if_needed()` — capture and preserve `restart_count`
- [ ] Fix `load_tts_backend()` Starting state — add `start_time: Instant::now()`
- [ ] Fix `load_tts_backend()` Ready state — add `restart_count: 0`
- [ ] Fix `cleanup_stale_processes()` in `server/mod.rs` — add `restart_count: 0`
- [ ] Fix test in `mod.rs` — add `restart_count: 0`
- [ ] Fix test in `status.rs` — add `restart_count: 0`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
  - Must succeed with zero errors
- [ ] Run `cargo test --package tama-core -- lifecycle::tests`
  - Existing tests should still pass
- [ ] Commit with message: "feat: set start_time and restart_count in all ModelState construction sites"

**Acceptance criteria:**
- [ ] All `ModelState::Starting` constructions include `start_time: Instant::now()`
- [ ] All `ModelState::Ready` constructions include `restart_count: 0`
- [ ] All `ModelState::Unloading` constructions preserve `restart_count`
- [ ] `cargo build --package tama-core` succeeds with zero errors
- [ ] All existing lifecycle tests pass

---

### Task 3: Extend check_idle_timeouts() with process health verification

**Context:**
This is the core of the feature. The `check_idle_timeouts()` method already runs periodically and handles Failed state cleanup + idle unloading. We extend it to:

1. **Ready models:** Check if the backend PID is still alive (fast `is_process_alive()` syscall). If dead, confirm via health endpoint (outside the lock). Then clean up and auto-restart (spawned). If `restart_count >= supervisor.max_restarts`, transition to Failed instead.
2. **Starting models:** Check if the model has been starting longer than `startup_timeout_secs`. If so, transition to Failed with a descriptive error.

**Critical lock discipline:**
- `is_process_alive()` (fast syscall) is called under the read lock — collects candidates only
- `check_health()` (HTTP request, up to 5s timeout) is called AFTER dropping the read lock
- Failed state insertion for max-restarts-exceeded happens under the SAME write lock as removal — no race window
- `load_model()` restart is spawned after all locks are released
- After successful auto-restart, `restart_count` is incremented in the new Ready state

**Restart count propagation:** The spawned restart task increments `restart_count` after `load_model()` succeeds. This is done by acquiring a write lock and updating the field in the Ready state. Without this, the counter would reset to 0 on each restart (since `load_model()` always creates `restart_count: 0`).

**Restart delay:** The spawned task respects `supervisor.restart_delay_ms` (default 3s) before calling `load_model()`, preventing rapid restart hammering.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle.rs`

**What to implement:**

Replace the current `check_idle_timeouts()` method. The extended version follows this flow:

**Phase 1: Collect candidates under read lock (fast only)**
```rust
// Collections
let mut to_unload = Vec::new();
let mut failed_to_remove = Vec::new();
// (server_name, model_name, backend, restart_count, pid, backend_url)
let mut dead_pid_candidates: Vec<(String, String, String, u32, u32, String)> = Vec::new();
// (server_name, model_name, backend)
let mut stuck_starting_servers: Vec<(String, String, String)> = Vec::new();

let (auto_unload, idle_timeout_secs, startup_timeout_secs, max_restarts, restart_delay_ms) = {
    let cfg = self.config.read().await;
    (
        cfg.proxy.auto_unload,
        cfg.proxy.idle_timeout_secs,
        cfg.proxy.startup_timeout_secs,
        cfg.supervisor.max_restarts,
        cfg.supervisor.restart_delay_ms,
    )
};

let idle_timeout = Duration::from_secs(idle_timeout_secs);
let startup_timeout = Duration::from_secs(startup_timeout_secs);

let models = self.models.read().await;
for (server_name, state) in models.iter() {
    // Check Starting state first (including TTS — they can also get stuck)
    if let ModelState::Starting { start_time, .. } = state {
        if now.saturating_duration_since(*start_time) > startup_timeout {
            warn!("Server '{}' stuck in Starting for {}s (timeout: {}s)",
                server_name, now.saturating_duration_since(*start_time).as_secs(), startup_timeout_secs);
            stuck_starting_servers.push((
                server_name.clone(),
                state.model_name().to_string(),
                state.backend().to_string(),
            ));
        }
        continue;
    }

    // Skip Unloading — already being handled
    if matches!(state, ModelState::Unloading { .. }) {
        continue;
    }

    // Skip TTS backends for Ready checks (they have separate lifecycle)
    // But TTS Starting was already checked above
    if state.is_tts_backend() {
        continue;
    }

    // Ready models — check PID liveness (fast syscall, OK under lock)
    if let ModelState::Ready { backend_pid, restart_count, .. } = state {
        let pid = *backend_pid;
        if !super::process::is_process_alive(pid) {
            // Collect for health check confirmation (done outside lock)
            dead_pid_candidates.push((
                server_name.clone(),
                state.model_name().to_string(),
                state.backend().to_string(),
                *restart_count,
                pid,
                state.backend_url().map(|u| u.to_string()).unwrap_or_default(),
            ));
            continue; // Skip idle check — process is dead
        }

        // Process alive — check idle timeout (existing logic)
        if let Some(last) = state.last_accessed() {
            let idle_duration = now.saturating_duration_since(last);
            if auto_unload && idle_duration > idle_timeout {
                to_unload.push(server_name.clone());
            }
        }
    }

    // Failed models — mark for cleanup
    if matches!(state, ModelState::Failed { .. }) {
        failed_to_remove.push(server_name.clone());
    }
}
drop(models); // Release read lock
```

**Phase 2: Health confirmation (outside lock)**
```rust
// Confirm dead PIDs via health endpoint — NO lock held
let mut confirmed_dead: Vec<(String, String, String, u32)> = Vec::new();
for (server_name, model_name, backend, restart_count, pid, backend_url) in dead_pid_candidates {
    let health_url = format!("{}/health", backend_url);
    let still_dead = match super::process::check_health(&health_url, Some(5)).await {
        Ok(resp) => !resp.status().is_success(),
        Err(_) => true, // Connection failed = definitely dead
    };

    if still_dead {
        info!("Server '{}' confirmed dead (pid {}, restart_count: {}/{})",
            server_name, pid, restart_count, max_restarts);
        confirmed_dead.push((server_name, model_name, backend, restart_count));
    } else {
        debug!("Server '{}' PID {} reused, health endpoint responds", server_name, pid);
    }
}
```

**Phase 3: Mutations (write locks, spawned tasks)**
```rust
// Remove Failed models
if !failed_to_remove.is_empty() {
    let mut models = self.models.write().await;
    for server_name in &failed_to_remove {
        models.remove(server_name);
    }
}

// Handle stuck Starting — transition to Failed
if !stuck_starting_servers.is_empty() {
    let mut models = self.models.write().await;
    for (server_name, model_name, backend) in &stuck_starting_servers {
        models.insert(server_name.clone(), ModelState::Failed {
            model_name: model_name.clone(),
            backend: backend.clone(),
            error: format!("Stuck in Starting state for {}s — backend failed to initialize", startup_timeout_secs),
        });
    }
}

// Handle dead Ready servers — clean up + insert Failed or spawn restart
if !confirmed_dead.is_empty() {
    // Remove from models map AND insert Failed states under the SAME lock — no race
    {
        let mut models = self.models.write().await;
        for (server_name, model_name, backend, restart_count) in &confirmed_dead {
            models.remove(server_name);
            if *restart_count >= max_restarts {
                // Insert Failed state immediately — no gap for races
                models.insert(server_name.clone(), ModelState::Failed {
                    model_name: model_name.clone(),
                    backend: backend.clone(),
                    error: format!("Exceeded maximum restart attempts ({}) — manual intervention required", max_restarts),
                });
                warn!("Server '{}' exceeded max restarts ({}/{})", server_name, restart_count, max_restarts);
            }
        }
    }
    // Clean DB entries (best-effort, only for non-Failed ones — Failed ones are cleaned on next tick)
    if let Some(conn) = self.open_db() {
        for (server_name, _, _, restart_count) in &confirmed_dead {
            if *restart_count < max_restarts {
                let _ = crate::db::queries::remove_active_model(&conn, server_name);
            }
        }
    }

    // Spawn restart tasks (no locks held)
    for (server_name, model_name, _, restart_count) in &confirmed_dead {
        if *restart_count >= max_restarts {
            continue; // Already inserted Failed state
        }

        let new_restart_count = restart_count + 1;
        info!("Auto-restarting '{}' (model '{}', attempt {}/{})",
            server_name, model_name, new_restart_count, max_restarts);

        let state = self.clone();
        let sn = server_name.clone();
        let mn = model_name.clone();
        let rdc = new_restart_count;
        tokio::spawn(async move {
            // Respect restart delay to prevent rapid hammering
            tokio::time::sleep(Duration::from_millis(restart_delay_ms)).await;
            match state.load_model(&mn, None).await {
                Ok(_) => {
                    // Increment restart_count in the new Ready state
                    let mut models = state.models.write().await;
                    if let Some(ModelState::Ready { restart_count: rc, .. }) = models.get_mut(&sn) {
                        *rc = rdc;
                    }
                    info!("Auto-restart succeeded for '{}' (model '{}')", sn, mn);
                }
                Err(e) => {
                    warn!("Auto-restart failed for '{}' (model '{}'): {}", sn, mn, e);
                }
            }
        });
    }
}

// Unload idle models (existing logic)
for server_name in &to_unload {
    if let Err(e) = self.unload_model(server_name).await {
        warn!("Failed to unload '{}': {}", server_name, e);
    }
}

// Build return value
let mut cleaned = Vec::new();
cleaned.extend(failed_to_remove);
cleaned.extend(stuck_starting_servers.iter().map(|(n, _, _)| n.clone()));
cleaned.extend(confirmed_dead.iter().map(|(n, _, _, _)| n.clone()));
cleaned.extend(to_unload);
cleaned
```

**Key design decisions:**
- `is_process_alive()` is fast (single syscall) — called under read lock is fine
- `check_health()` is slow (HTTP, up to 5s) — called AFTER dropping read lock
- Failed state for max-restarts is inserted under the SAME write lock as removal — no race window
- `restart_count` is incremented AFTER `load_model()` succeeds by updating the Ready state in-place
- `supervisor.restart_delay_ms` is respected inside the spawned task (before `load_model()`)
- Backend name is included in all Failed states (not `String::new()`)
- TTS Starting state IS checked (moved before the TTS skip)

**Steps:**
- [ ] Update test helpers first (from Task 4) — `make_ready_state()`, `make_starting_state()`, `make_unloading_state()`, plus new helpers `make_ready_state_with_restarts()` and `make_starting_state_with_time()`
- [ ] Write failing test: `test_dead_pid_detected_and_restarted`
  - Insert a Ready model with PID 999999 (definitely dead)
  - Call `check_idle_timeouts()`
  - Verify the model is removed from the models map
  - Run `cargo test --package tama-core test_dead_pid -- --nocapture`
  - Must fail because dead PID check doesn't exist yet
- [ ] Write failing test: `test_stuck_starting_server_marked_failed`
  - Insert a Starting model with `start_time` set 300s in the past
  - Call `check_idle_timeouts()`
  - Verify the model transitions to Failed state
  - Run `cargo test --package tama-core test_stuck_starting -- --nocapture`
- [ ] Write failing test: `test_max_restarts_exceeded_goes_to_failed`
  - Set config `supervisor.max_restarts = 2`
  - Insert a Ready model with `restart_count: 2` and PID 999999
  - Call `check_idle_timeouts()`
  - Verify the model is in Failed state (not restarted)
  - Run `cargo test --package tama-core test_max_restarts -- --nocapture`
- [ ] Implement the extended `check_idle_timeouts()` as described above
- [ ] Run `cargo test --package tama-core -- lifecycle::tests`
  - All tests must pass (existing + new)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add process health monitoring to check_idle_timeouts"

**Acceptance criteria:**
- [ ] Ready models with dead PIDs are detected, confirmed via health endpoint, cleaned from DB, and auto-restarted (spawned)
- [ ] `restart_count` is incremented after successful auto-restart (not reset to 0)
- [ ] `supervisor.restart_delay_ms` is respected before restart
- [ ] Starting models exceeding `startup_timeout_secs` transition to Failed state (including TTS)
- [ ] Models exceeding `supervisor.max_restarts` go to Failed (inserted under same lock as removal — no race)
- [ ] Health endpoint double-check prevents false positives from PID reuse
- [ ] All existing `check_idle_timeouts` tests still pass
- [ ] New tests pass: dead PID detection, stuck Starting, max restarts exceeded
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 4: Update test helpers for new ModelState fields

**Context:**
The `lifecycle.rs` test module has helper functions that construct `ModelState` variants for testing. These need the new fields. This task MUST be done before Task 3's tests because the failing tests in Task 3 depend on these helpers.

**NOTE:** This task should be executed BEFORE Task 3's test steps. The task ordering in the plan lists Task 3 before Task 4 for logical grouping, but the executing agent should do Task 4's helper updates first, then write Task 3's failing tests.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle.rs` (test module at bottom of file)

**What to implement:**

1. Update `make_ready_state()` helper to include `restart_count: 0`:
   ```rust
   fn make_ready_state(model_name: &str, backend: &str) -> ModelState {
       ModelState::Ready {
           model_name: model_name.to_string(),
           backend: backend.to_string(),
           backend_pid: 12345,
           backend_url: "http://127.0.0.1:8080".to_string(),
           load_time: std::time::SystemTime::now(),
           last_accessed: Instant::now(),
           consecutive_failures: Arc::new(AtomicU32::new(0)),
           failure_timestamp: None,
           restart_count: 0,  // NEW
       }
   }
   ```

2. Update `make_starting_state()` helper to include `start_time: Instant::now()`:
   ```rust
   fn make_starting_state(model_name: &str, backend: &str) -> ModelState {
       ModelState::Starting {
           model_name: model_name.to_string(),
           backend: backend.to_string(),
           backend_url: String::new(),
           last_accessed: Instant::now(),
           start_time: Instant::now(),  // NEW
           consecutive_failures: Arc::new(AtomicU32::new(0)),
           failure_timestamp: None,
       }
   }
   ```

3. Update `make_unloading_state()` helper to include `restart_count: 0`:
   ```rust
   fn make_unloading_state(model_name: &str, backend: &str) -> ModelState {
       ModelState::Unloading {
           model_name: model_name.to_string(),
           backend: backend.to_string(),
           backend_pid: 54321,
           backend_url: "http://127.0.0.1:9000".to_string(),
           last_accessed: Instant::now(),
           consecutive_failures: Arc::new(AtomicU32::new(0)),
           failure_timestamp: None,
           restart_count: 0,  // NEW
       }
   }
   ```

4. Add helper for custom restart count:
   ```rust
   fn make_ready_state_with_restarts(model_name: &str, backend: &str, restart_count: u32) -> ModelState {
       let mut state = make_ready_state(model_name, backend);
       if let ModelState::Ready { restart_count: rc, .. } = &mut state {
           *rc = restart_count;
       }
       state
   }
   ```

5. Add helper for custom start_time:
   ```rust
   fn make_starting_state_with_time(model_name: &str, backend: &str, start_time: Instant) -> ModelState {
       let mut state = make_starting_state(model_name, backend);
       if let ModelState::Starting { start_time: st, .. } = &mut state {
           *st = start_time;
       }
       state
   }
   ```

**Steps:**
- [ ] Update `make_ready_state()` to include `restart_count: 0`
- [ ] Update `make_starting_state()` to include `start_time: Instant::now()`
- [ ] Update `make_unloading_state()` to include `restart_count: 0`
- [ ] Add `make_ready_state_with_restarts()` helper
- [ ] Add `make_starting_state_with_time()` helper
- [ ] Run `cargo test --package tama-core -- lifecycle::tests`
  - All existing tests should still pass
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "test: update ModelState test helpers for new fields"

**Acceptance criteria:**
- [ ] All test helpers include new fields with correct defaults
- [ ] Helper functions for custom restart_count and start_time exist
- [ ] All lifecycle tests pass: `cargo test --package tama-core -- lifecycle::tests`

---

### Task 5: Verify web UI does not expose internal ModelState fields

**Context:**
The new fields (`start_time`, `restart_count`) are internal runtime state on `ModelState`. They must NOT appear in the web UI config editor or any API response. This is a verification-only task — no code changes expected.

**Files:**
- Review: `crates/tama-web/src/types/config.rs`
- Review: `crates/tama-web/src/pages/config_editor.rs`

**What to implement:**

1. Verify that `start_time` and `restart_count` are NOT exposed in any web UI config type. `ModelState` is defined in `tama-core`, not `tama-web`, so these fields are inherently internal.

2. Check if `supervisor.max_restarts` is exposed in the web UI. If it is, no changes needed — the field's purpose will be self-evident from the feature working.

3. No code changes expected. This is a verification checkpoint.

**Steps:**
- [ ] Confirm `crates/tama-web/src/types/config.rs` does not import or reference `ModelState`
- [ ] Confirm `crates/tama-web/src/pages/config_editor.rs` does not display `start_time` or `restart_count`
- [ ] If any changes are needed (unlikely), implement them
- [ ] If no changes needed, skip the commit (no empty commits)

**Acceptance criteria:**
- [ ] `start_time` and `restart_count` are NOT exposed in web UI
- [ ] No code changes needed (verification passes)

---

### Task 6: Full workspace build and test

**Context:**
Final verification that the entire workspace builds and all tests pass after the changes.

**Files:**
- All modified files from previous tasks

**Steps:**
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Fix any warnings
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
  - Fix any failing tests
- [ ] Run `cargo test --workspace` again to confirm all pass
- [ ] Commit with message: "fix: resolve clippy warnings and test failures for process health monitor"

**Acceptance criteria:**
- [ ] `cargo fmt --all` makes no changes
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes with zero failures

---

## Summary of Changes

| File | Change |
|------|--------|
| `crates/tama-core/src/proxy/types.rs` | Add `start_time` to Starting, `restart_count` to Ready/Unloading; add helper methods |
| `crates/tama-core/src/proxy/lifecycle.rs` | Update all construction sites for new fields; extend `check_idle_timeouts()` with PID verification, health confirmation, auto-restart with delay, restart_count propagation, Starting timeout guard; update test helpers + 3 new tests |
| `crates/tama-core/src/proxy/server/mod.rs` | Add `restart_count: 0` to `cleanup_stale_processes()` Ready construction |
| `crates/tama-core/src/proxy/mod.rs` | Add `restart_count: 0` to test code |
| `crates/tama-core/src/proxy/status.rs` | Add `restart_count: 0` to test code |

## Task Execution Order

1. Task 1: Add fields to ModelState (compile errors expected)
2. Task 4: Update test helpers (needed before Task 3's tests)
3. Task 2: Fix all construction sites (resolve compile errors)
4. Task 3: Extend check_idle_timeouts() (core feature + new tests)
5. Task 5: Verify web UI (verification only)
6. Task 6: Full workspace build and test

## Known Limitations

- **PID reuse on Linux:** After suspend/resume, a PID could theoretically be reused by a different process. Mitigated by double-checking via the health endpoint (which is model-specific). If the PID is reused AND the new process happens to respond to `/health`, the false positive would persist until a request triggers the circuit breaker.
- **Restart count is in-memory only:** A proxy restart clears `restart_count`. A flapping model could restart infinitely across proxy restarts. Mitigated by the circuit breaker (which also resets on proxy restart) — this is an accepted trade-off for simplicity.
- **Auto-restart is best-effort:** The spawned `load_model()` task is fire-and-forget. If it fails, the model goes to Failed state and requires manual intervention.
- **`check_idle_timeouts()` return value semantics:** The return value now includes all cleaned servers (Failed, stuck Starting, dead Ready, idle unloaded), not just idle-unloaded servers. The current caller discards the return value, so this is safe.
