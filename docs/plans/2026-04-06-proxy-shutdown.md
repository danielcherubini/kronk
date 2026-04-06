# Implement ProxyState::shutdown Plan

**Goal:** Implement a graceful shutdown method for `ProxyState` to allow clean process exit.

**Status:** ✅ COMPLETED - See git commits `6c83743` ("feat: add shutdown method to ProxyState for graceful exit"), `82ec8ab` ("fix(proxy): fix system restart handler and shutdown")
**Architecture:** Add an `async fn shutdown(&self)` method to `ProxyState`. The method will unload all models, close the metrics broadcast channel, and attempt to cancel active pull jobs.
**Tech Stack:** Rust, Tokio, Koji Core.

---

### Task 1: Implement `ProxyState::shutdown` and Tests

**Context:**
Currently, `ProxyState` manages the lifecycle of models and metrics but lacks a centralized way to shut down all active components. This is needed for a graceful shutdown of the Koji proxy.

**Files:**
- Modify: `crates/koji-core/src/proxy/lifecycle.rs` (to add the implementation)
- Modify: `crates/koji-core/src/proxy/mod.rs` (to add tests)

**What to implement:**
Add `async fn shutdown(&self)` to `impl ProxyState` in `crates/koji-core/src/proxy/lifecycle.rs`.
The method must:
1. Iterate over `self.models` (using `read().await`) and for each model, call `self.unload_model(name).await`.
2. Signal the `metrics_tx` (broadcast channel) to close, effectively stopping the metrics stream. Note: In Tokio's broadcast, dropping the sender or sending a message that signals shutdown to receivers is one way. Given the requirement, we'll look for the best way to signal it via the `metrics_tx`.
3. Attempt to cancel/wait for active `pull_jobs` if possible. We'll clear the `pull_jobs` map.

**Steps:**
- [ ] Write failing test for `ProxyState::shutdown` in `crates/koji-core/src/proxy/mod.rs`
- [ ] Run `cargo test --package koji-core`
  - Did it fail with `method not found`? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `shutdown` in `crates/koji-core/src/proxy/lifecycle.rs`
- [ ] Run `cargo test --package koji-core`
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat: implement ProxyState::shutdown"

**Acceptance criteria:**
- [ ] `shutdown()` method exists and is callable.
- [ ] `shutdown()` successfully calls `unload_model` for all loaded models.
- [ ] `shutdown()` handles metrics signaling.
- [ ] `shutdown()` handles pull jobs.
