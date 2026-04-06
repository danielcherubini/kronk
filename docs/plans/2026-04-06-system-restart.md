# Implement System Restart Logic (Process-Level Exit)

**Goal:** Implement a functional system restart by triggering a graceful shutdown and process exit.
**Architecture:** The restart handler will trigger a shutdown sequence in the `ProxyState` to clean up resources (models, jobs) and then terminate the process. The host environment (systemd, Docker, etc.) is expected to handle the process recreation.
**Tech Stack:** Rust, Axum, Tokio, Koji Core.

---

### Task 1: Implement `shutdown` method in `ProxyState`

**Context:**
Currently, the `handle_koji_system_restart` function is a placeholder. To perform a hard restart, we must first ensure the application cleans up its resources (unloads models, closes channels) before the process exits.

**Files:**
- Modify: `crates/koji-core/src/proxy/mod.rs` (or the file containing `ProxyState` definition)

**What to implement:**
- Add an `async fn shutdown(&self)` method to the `ProxyState` struct.
- The method should:
    1. Iterate through all currently loaded models in `self.models` and call their respective unload logic.
    2. Signal the `metrics_tx` (broadcast channel) to close, effectively stopping the metrics stream.
    3. Attempt to cancel/wait for active `pull_jobs` if possible.
- Ensure the method is thread-safe and handles the `Arc<RwLock<...>>` patterns correctly.

**Steps:**
- [ ] Write a unit test in `crates/koji-core/src/proxy/mod.rs` that verifies `shutdown` is called and cleans up the `models` map.
- [ ] Run `cargo test -p koji-core`
  - Did it fail? If so, fix it.
- [ ] Implement `shutdown` in `ProxyState`.
- [ ] Run `cargo test -p koji-core`
  - Did it pass?
- [ ] Run `cargo fmt`
- [ ] Run `cargo check`
- [ ] Commit with message: "feat: add shutdown method to ProxyState for graceful exit"

**Acceptance criteria:**
- [ ] `ProxyState::shutdown` exists and is `async`.
- [ ] `ProxyState::shutdown` successfully clears or handles loaded models.
- [ ] The method is accessible via the `Arc<ProxyState>` handle.

---

### Task 2: Update the Restart Handler to trigger exit

**Context:**
The API endpoint `/koji/v1/system/restart` needs to be wired up to the new `shutdown` logic and actually terminate the process.

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs`

**What to implement:**
- Update the `handle_koji_system_restart` function.
- The function should:
    1. Call `state.shutdown().await`.
    2. After the shutdown sequence completes, call `std::process::exit(0)`.
- Return a `202 Accepted` or `200 OK` response if possible, though the process will terminate quickly.

**Steps:**
- [ ] Write a test case that calls the handler and checks if it initiates the shutdown logic.
- [ ] Run `cargo test -p koji-core`
- [ ] Implement the logic in `handle_koji_system_restart`.
- [ ] Run `cargo test -p koji-core`
- [ ] Run `cargo fmt`
- [ ] Run `cargo check`
- [ ] Commit with message: "feat: implement system restart handler with process exit"

**Acceptance criteria:**
- [ ] Calling `POST /koji/v1/system/restart` triggers the shutdown sequence.
- [ ] The process exits with code 0.

---

### Task 3: Integration Test for Process Termination

**Context:**
We need to verify that a real call to the API results in the process actually exiting.

**Files:**
- Create: `crates/koji-core/src/proxy/tests/restart_test.rs` (or add to existing proxy tests)

**What to implement:**
- An integration test that:
    1. Spawns the Koji proxy server in a background task.
    2. Uses a `reqwest` client to send a `POST` request to the restart endpoint.
    3. Uses a timeout/watcher to detect if the server process has terminated.
- *Note: We'll use `std::process::Command` to run the binary directly to avoid the test runner exiting with the application.*

**Steps:**
- [ ] Implement the test using `std::process::Command` to run the compiled binary.
- [ ] Run the test.
- [ ] Run `cargo fmt`
- [ ] Run `cargo check`
- [ ] Commit with message: "test: add integration test for system restart exit"

**Acceptance criteria:**
- [ ] The test passes, confirming that the binary terminates upon receiving the restart command.
