# Fix: Startup Detection & Orphaned Process Cleanup

**Goal:** Fix two bugs — (1) model startup detection missing healthy backends, (2) orphaned child processes left running when startup fails.

**Architecture:** Require 2 consecutive health checks before marking a backend healthy, and use process groups (Unix) / process trees (Windows) so that on startup failure the entire process tree is killed.

**Tech Stack:** Rust, libc (Unix process groups), tokio-process

---

### Task 1: Add `backend_pid` to `Starting` state

**Context:**
The `ModelState::Starting` variant currently has no PID field. This means when startup fails and the state is transitioned to `Failed`, there is no way to look up and kill the orphaned backend process from any code path (e.g., `check_idle_timeouts()`). Adding the PID to `Starting` enables cleanup from any code that inspects the state map.

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle.rs` (test helper `make_starting_state`)
- Modify: `crates/tama-core/src/proxy/status.rs` (test at line ~448)
- Modify: `crates/tama-core/src/proxy/forward.rs` (no changes needed — uses `..` match arms)

**What to implement:**

1. In `types.rs`, add `backend_pid: u32` field to the `Starting` variant:

```rust
Starting {
    model_name: String,
    backend: String,
    backend_url: String,
    backend_pid: u32,              // NEW FIELD
    last_accessed: Instant,
    start_time: Instant,
    consecutive_failures: Arc<AtomicU32>,
    failure_timestamp: Option<SystemTime>,
}
```

2. In `types.rs`, update the `backend_pid()` method to return the PID for `Starting`:

```rust
pub fn backend_pid(&self) -> Option<u32> {
    match self {
        ModelState::Starting { backend_pid, .. } => Some(*backend_pid),
        ModelState::Ready { backend_pid, .. } => Some(*backend_pid),
        ModelState::Unloading { backend_pid, .. } => Some(*backend_pid),
        _ => None,
    }
}
```

3. In `lifecycle.rs`, update BOTH places where `ModelState::Starting` is constructed to include `backend_pid: pid`:
   - `load_model()` ~line 52 — the main `models.insert(server_name, ModelState::Starting { ... })` call
   - `load_tts_backend()` ~line 808 — the TTS Starting state insertion

   The Starting state is currently inserted **before** spawn (to guard against concurrent `load_model()` calls). The PID is only known **after** spawn. Resolution:
   - Insert `Starting` with `backend_pid: 0` as placeholder (process not yet spawned)
   - After successful spawn, update the PID via `get_mut()` under the write lock
   - The PID=0 window is microseconds (spawn → PID update is synchronous), and `check_idle_timeouts()` only marks as stuck after `startup_timeout` (120s default), so there is no practical race

   Code for the PID update (add after `let pid = child.id()...`):
   ```rust
   // Update the PID in the Starting state so cleanup paths can find it
   {
       let mut models = self.models.write().await;
       if let Some(ModelState::Starting { backend_pid, .. }) = models.get_mut(&server_name) {
           *backend_pid = pid;
       }
   }
   ```

4. In `lifecycle.rs` test module, update `make_starting_state()` helper (~line 1042) to include `backend_pid: 0`.

5. In `status.rs` test module (~line 448), update the inline `ModelState::Starting` construction to include `backend_pid: 0`.

6. All `match` arms on `Starting` throughout the codebase already use `{ field, .. }` or `{ .. }` patterns, so they compile without changes. Verify:
   - `forward.rs` line 301: `ModelState::Starting { failure_timestamp, .. }` — no change
   - `state.rs` line 160: `ModelState::Starting { last_accessed, .. }` — no change
   - `lifecycle.rs` line 235: `ModelState::Starting { consecutive_failures, failure_timestamp, .. }` — no change
   - `status.rs` line 137: `ModelState::Starting { consecutive_failures, .. }` — no change

**Steps:**
- [ ] Add `backend_pid: u32` to `Starting` in `types.rs`
- [ ] Update `backend_pid()` method in `types.rs` to return Some for Starting
- [ ] Update `load_model()` to insert Starting with `backend_pid: 0`, then update after spawn
- [ ] Update `load_tts_backend()` to insert Starting with `backend_pid: 0`, then update after spawn
- [ ] Update `make_starting_state()` test helper in `lifecycle.rs`
- [ ] Update inline Starting construction in `status.rs` test
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix missing field errors and re-run
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run
- [ ] Commit with message: "feat: add backend_pid to ModelState::Starting for orphan cleanup"

**Acceptance criteria:**
- [ ] `ModelState::Starting` has a `backend_pid` field
- [ ] `backend_pid()` returns `Some(pid)` for Starting states
- [ ] `load_model()` stores the spawned PID in Starting state after spawn
- [ ] `load_tts_backend()` stores the spawned PID in Starting state after spawn
- [ ] All tests compile and pass

---

### Task 2: Add process group functions to `proxy/process.rs`

**Context:**
When a backend process spawns child processes (GPU helpers, worker threads, etc.), killing only the parent PID leaves orphaned children. Process groups on Unix (and process trees on Windows) let us kill the entire tree with one signal.

**Files:**
- Modify: `crates/tama-core/src/proxy/process.rs`

**What to implement:**

Add 4 new public functions to `crates/tama-core/src/proxy/process.rs`:

1. **`configure_process_group(cmd: &mut tokio::process::Command)`** — called before spawning to create a new process group:

```rust
/// Configure a child process to be spawned in its own process group.
/// On Unix, uses process_group(0) to create a new session.
/// On Windows, uses CREATE_NEW_PROCESS_GROUP flag.
/// Call this before spawning any backend process.
pub fn configure_process_group(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.process_group(0);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP = 0x00000200
        cmd.creation_flags(0x00000200);
    }
}
```

2. **`kill_process_group(pid: u32) -> Result<()>`** — send SIGTERM to the entire process group:

```rust
/// Send SIGTERM to an entire process group (Unix) or kill the process tree (Windows).
/// On Unix, negative PID in kill() targets the process group.
/// On Windows, delegates to kill_process() which uses taskkill /T (tree kill).
pub async fn kill_process_group(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGTERM) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            // ESRCH = no such process group, which is fine (already dead)
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(anyhow::anyhow!(
                    "Failed to send SIGTERM to process group {}: {}",
                    pid, err
                ));
            }
        }
    }
    #[cfg(windows)]
    {
        kill_process(pid).await?;
    }
    Ok(())
}
```

3. **`force_kill_process_group(pid: u32) -> Result<()>`** — send SIGKILL to the entire process group:

```rust
/// Send SIGKILL to an entire process group (Unix) or force-kill the process tree (Windows).
/// On Windows, delegates to force_kill_process() which uses taskkill /T /F (forceful tree kill).

```rust
/// Send SIGKILL to an entire process group (Unix) or force-kill the process tree (Windows).
/// On Windows, delegates to force_kill_process() which uses taskkill /T /F (forceful tree kill).
pub async fn force_kill_process_group(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(anyhow::anyhow!(
                    "Failed to send SIGKILL to process group {}: {}",
                    pid, err
                ));
            }
        }
    }
    #[cfg(windows)]
    {
        force_kill_process(pid).await?;
    }
    Ok(())
}
```

4. **`is_process_group_alive(pid: u32) -> bool`** — check if the process group leader is still alive:

```rust
/// Check if a process group leader (by PID) is still alive.
/// If the leader is dead, the group is effectively dead.
pub fn is_process_group_alive(pid: u32) -> bool {
    is_process_alive(pid)
}
```

**Note:** `libc` is already an unconditional dependency in `Cargo.toml` (`libc = "0.2.183"`), so no Cargo.toml changes are needed. The `#[cfg(unix)]` guards in the code are sufficient.

**Steps:**
- [ ] Implement `configure_process_group()` in `proxy/process.rs`
- [ ] Implement `kill_process_group()` in `proxy/process.rs`
- [ ] Implement `force_kill_process_group()` in `proxy/process.rs`
- [ ] Implement `is_process_group_alive()` in `proxy/process.rs`
- [ ] Write test `test_kill_process_group_nonexistent_pid_returns_ok()` in `proxy/process.rs` — verify ESRCH is handled gracefully (returns Ok, not Err)
- [ ] Write integration test `test_process_group_kills_children()` that spawns a parent process that forks a child, verifies both are killed by `kill_process_group()` (use `/bin/sh -c 'sleep 100 &'` as the child spawner)
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run
- [ ] Commit with message: "feat: add process group management functions"

**Acceptance criteria:**
- [ ] `configure_process_group()` sets up a new process group (Unix: process_group(0), Windows: CREATE_NEW_PROCESS_GROUP)
- [ ] `kill_process_group()` sends SIGTERM to the process group on Unix
- [ ] `force_kill_process_group()` sends SIGKILL to the process group on Unix
- [ ] `is_process_group_alive()` wraps `is_process_alive()`
- [ ] All functions handle edge cases (process already dead = ESRCH is fine)
- [ ] Tests verify ESRCH handling and child process cleanup

---

### Task 3: Update `load_model()` — 2-consecutive health checks + process group + cleanup

**Context:**
The `load_model()` function has two bugs: (1) it breaks on the first successful health check, which can miss genuinely healthy backends, and (2) on startup timeout it only kills the parent PID, leaving child processes orphaned. This task fixes both by requiring 2 consecutive health checks and using process group cleanup.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle.rs`

**What to implement:**

1. **Add process group setup** — after `configure_backend_command()`, add `configure_process_group()`:

```rust
let mut child = tokio::process::Command::new(&backend_path);
crate::process::configure_backend_command(&mut child, &backend_path);
super::process::configure_process_group(&mut child);  // NEW
child
```

2. **Update health check loop** — change from "break on first success" to "require 2 consecutive successes":

Replace the existing loop:
```rust
let start = Instant::now();
let mut health_ok = false;

loop {
    tokio::time::sleep(Duration::from_millis(500)).await;
    if start.elapsed() >= timeout {
        let _ = kill_process(pid).await;
        break;
    }
    if let Ok(response) = super::process::check_health(&health_url, Some(30)).await {
        if response.status().is_success() {
            debug!("Health check passed for server: {}", server_name);
            health_ok = true;
            break;
        }
    }
}
```

With:
```rust
let start = Instant::now();
let mut consecutive_successes = 0u32;
let mut health_ok = false;

loop {
    tokio::time::sleep(Duration::from_millis(500)).await;
    if start.elapsed() >= timeout {
        warn!(
            "Startup health check timeout for server '{}' after {}s, killing process group",
            server_name, timeout.as_secs()
        );
        // Kill entire process group, not just parent
        let _ = super::process::kill_process_group(pid).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        if super::process::is_process_group_alive(pid) {
            warn!("Process group {} still alive, sending SIGKILL", pid);
            let _ = super::process::force_kill_process_group(pid).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        break;
    }

    if let Ok(response) = super::process::check_health(&health_url, Some(30)).await {
        if response.status().is_success() {
            consecutive_successes += 1;
            if consecutive_successes >= 2 {
                debug!(
                    "Health check confirmed for server '{}' ({} consecutive successes)",
                    server_name, consecutive_successes
                );
                health_ok = true;
                break;
            }
            debug!(
                "Health check passed for server '{}' ({}/2 consecutive)",
                server_name, consecutive_successes
            );
        } else {
            consecutive_successes = 0;
        }
    } else {
        consecutive_successes = 0;
    }
}
```

3. **Import the new functions** at the top of the file:

```rust
use super::process::{force_kill_process, is_process_alive, kill_process, override_arg,
    configure_process_group, kill_process_group, force_kill_process_group, is_process_group_alive};
```

Or update the existing import line.

**Steps:**
- [ ] Update imports in `lifecycle.rs` to include new process group functions
- [ ] Add `configure_process_group()` call before spawn in `load_model()`
- [ ] After spawn, update the PID in the Starting state (add the code block from Task 1)
- [ ] Replace the health check loop with the 2-consecutive-successes version (keep 500ms polling interval)
- [ ] Replace `kill_process(pid)` on timeout with `kill_process_group(pid)` + SIGKILL escalation
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "fix: require 2 consecutive health checks and use process group cleanup in load_model"

**Acceptance criteria:**
- [ ] Health check requires 2 consecutive successes (1 second apart) before marking healthy
- [ ] Any failure resets the consecutive counter to 0
- [ ] On timeout, `kill_process_group()` is called with SIGKILL escalation
- [ ] `configure_process_group()` is called before spawning

---

### Task 4: Update `load_tts_backend()` — same changes

**Context:**
The `load_tts_backend()` function has the same two bugs as `load_model()`. This task applies the identical fixes to the TTS startup path.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle.rs`

**What to implement:**

Apply the same changes as Task 3, but to `load_tts_backend()`:

1. Add `configure_process_group()` call before spawn (~line 832):

```rust
let mut child = tokio::process::Command::new(&python_bin);
super::process::configure_process_group(&mut child);  // NEW
child
```

Note: `load_tts_backend()` does NOT call `crate::process::configure_backend_command()` because it spawns a Python venv interpreter (not a custom backend binary). Only add `configure_process_group()`.

2. After spawn, update the PID in the Starting state (same pattern as Task 3).

3. Update the health check loop from 2-second polling with single-success break to 500ms polling with 2-consecutive successes.

4. Replace `kill_process(pid)` on timeout with `kill_process_group(pid)` + SIGKILL escalation.

The current loop (~line 868):
```rust
loop {
    tokio::time::sleep(Duration::from_secs(2)).await;
    if start.elapsed() >= timeout {
        let _ = kill_process(pid).await;
        break;
    }
    if let Ok(response) = super::process::check_health(&health_url, Some(30)).await {
        if response.status().is_success() {
            debug!("Health check passed for TTS backend: {}", backend_name);
            health_ok = true;
            break;
        }
    }
}
```

Replace with the same 2-consecutive-successes pattern as Task 3 (polling at 500ms).

**Steps:**
- [ ] Add `configure_process_group()` before spawn in `load_tts_backend()`
- [ ] After spawn, update the PID in the Starting state (same pattern as Task 3)
- [ ] Replace health check loop with 2-consecutive-successes version (500ms polling)
- [ ] Replace `kill_process(pid)` on timeout with process group kill + SIGKILL escalation
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "fix: require 2 consecutive health checks and use process group cleanup in load_tts_backend"

**Acceptance criteria:**
- [ ] Same 2-consecutive health check behavior as `load_model()`
- [ ] Process group cleanup on timeout
- [ ] Builds and tests pass

---

### Task 5: Update `check_idle_timeouts()` — kill orphaned Starting processes

**Context:**
The `check_idle_timeouts()` method detects stuck `Starting` servers (past the startup timeout) and transitions them to `Failed`. With the PID now stored in `Starting`, we can also clean up the orphaned process group at this point, preventing zombie processes that would otherwise require manual cleanup.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle.rs`

**What to implement:**

In `check_idle_timeouts()`, in Phase 1 where stuck Starting servers are collected, also collect the PID. In Phase 3 where they're transitioned to Failed, also kill the process group.

1. Update the stuck Starting collection in Phase 1 to include the PID:

Change:
```rust
let mut stuck_starting_servers: Vec<(String, String, String, Instant)> = Vec::new();
```

To:
```rust
let mut stuck_starting_servers: Vec<(String, String, String, Instant, u32)> = Vec::new();
```

And the push:
```rust
stuck_starting_servers.push((
    server_name.clone(),
    state.model_name().to_string(),
    state.backend().to_string(),
    *start_time,
    state.backend_pid().unwrap_or(0),  // PID now stored in Starting
));
```

2. In Phase 3, update the `for` loop destructuring to include the PID:

Change:
```rust
for (server_name, model_name, backend, observed_start) in &stuck_starting_servers {
```

To:
```rust
for (server_name, model_name, backend, observed_start, observed_pid) in &stuck_starting_servers {
```

3. Collect PIDs to clean during Phase 3, then kill outside the write lock.

During Phase 3, collect `(server_name, pid)` tuples:
```rust
let mut stuck_pids_to_clean: Vec<(String, u32)> = Vec::new();
```

Add each one inside the Phase 3 lock block before transitioning to Failed:
```rust
stuck_pids_to_clean.push((server_name.clone(), *observed_pid));
```

After Phase 3 lock is released, kill orphaned process groups:
```rust
for (server_name, pid) in &stuck_pids_to_clean {
    if *pid > 0 {
        warn!("Killing orphaned process group {} for stuck server '{}'", pid, server_name);
        let _ = super::process::kill_process_group(*pid).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        if super::process::is_process_group_alive(*pid) {
            let _ = super::process::force_kill_process_group(*pid).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}
```
    }
}
```

3. Also add the new imports if not already present from Task 3.

**Steps:**
- [ ] Update `stuck_starting_servers` tuple to include PID (5 elements instead of 4)
- [ ] Update the `for` loop destructuring to include `observed_pid`
- [ ] Collect `(server_name, pid)` tuples during Phase 3 for cleanup
- [ ] Kill process groups outside the write lock after Phase 3
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "fix: kill orphaned process groups for stuck Starting servers in check_idle_timeouts"

**Acceptance criteria:**
- [ ] Stuck Starting servers have their process groups killed when transitioned to Failed
- [ ] Kill happens outside the write lock (no blocking)
- [ ] All existing tests pass

---

### Task 6: Verification

**Context:**
Final verification that all changes work together and no regressions were introduced.

**Files:**
- All files from Tasks 1-5

**Steps:**
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix all warnings
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run
- [ ] Run `cargo build --release --workspace`
- [ ] Commit with message: "chore: final verification and formatting for startup detection + orphan cleanup"

**Acceptance criteria:**
- [ ] `cargo check --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo build --release --workspace` passes

---

### Scope Notes

**In scope:**
- Startup detection (2-consecutive health checks)
- Orphaned process cleanup on startup failure
- Orphaned process cleanup for stuck Starting servers

**Out of scope (future work):**
- `unload_model()` and `unload_tts_backend()` still use `kill_process()` / `force_kill_process()` instead of process group variants. In practice, backends exit cleanly on SIGTERM and children follow, so this is low risk. A follow-up can migrate these to `kill_process_group()` / `force_kill_process_group()` for completeness.
