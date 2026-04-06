# Status Command Redesign - Implementation Plan

**Goal:** Unify `koji status` and `koji model ps` into a single fast command backed by a new proxy `/status` endpoint.
**Architecture:** Add `GET /status` to the proxy server that returns all model state, config, VRAM, and metrics in one JSON call. Rewrite `cmd_status` to consume this endpoint with a 500ms timeout, falling back to config-only display. Remove `model ps` entirely.
**Tech Stack:** Rust, axum, reqwest, serde_json, tokio (spawn_blocking for VRAM)

**Status:** ✅ COMPLETED - See git commits `4de3b5a` ("feat: unified status command, remove model ps"), `b077271` ("feat: add /status endpoint to proxy server"), `7a49b44` ("fix: move DB query outside loop in status.rs")

---

### Task 1: Add `GET /status` endpoint to proxy server

**Files:**
- Modify: `crates/koji-core/src/proxy.rs`
- Modify: `crates/koji-core/src/proxy/server.rs`

**Steps:**
- [ ] Add `build_status_response(&self) -> serde_json::Value` method to `ProxyState` in `proxy.rs`
  - Query VRAM via `tokio::task::spawn_blocking(|| gpu::query_vram())`
  - Iterate `self.config.models` for all configured models
  - For each model, resolve backend path from `self.config.backends`
  - For each model, check `self.models` (read lock) for loaded state
  - For loaded models: compute `last_accessed_secs_ago` from `Instant::now() - last_accessed`, `idle_timeout_remaining_secs` from `idle_timeout_secs - elapsed`, include `backend_pid`, `load_time` as unix timestamp, `consecutive_failures`
  - For unloaded models: null for runtime fields
  - Include `idle_timeout_secs` from `self.config.proxy`
  - Include proxy metrics from `self.metrics`
- [ ] Add `handle_status` async handler in `server.rs`
  - Call `state.build_status_response().await`
  - Return `Json(response)`
- [ ] Register `.route("/status", get(handle_status))` in `into_router()`
- [ ] Run `cargo build --workspace` to verify compilation
- [ ] Commit

### Task 2: Rewrite `cmd_status` in CLI

**Files:**
- Modify: `crates/koji-cli/src/main.rs`

**Steps:**
- [ ] Rewrite `cmd_status` to:
  - Build proxy URL from `config.proxy.host` and `config.proxy.port` + `/status`
  - Create `reqwest::Client` with 500ms timeout
  - Attempt GET to proxy `/status`
  - If successful: parse JSON, display VRAM at top, then each model with full details including loaded/idle info
  - If failed (connection refused, timeout): fall back to config-only display
    - Query VRAM locally via `gpu::query_vram()`
    - Iterate `config.models` and show config fields with `Loaded: proxy not running`
- [ ] Add helper `format_idle_time(secs: u64) -> String` to format seconds as `Xm Ys` or `Xs`
- [ ] Remove all `platform::windows::query_service` / `platform::linux::query_service` calls from `cmd_status`
- [ ] Remove the per-model HTTP health check loop
- [ ] Run `cargo build --workspace` to verify compilation
- [ ] Commit

### Task 3: Remove `koji model ps` command

**Files:**
- Modify: `crates/koji-cli/src/commands/model.rs`
- Modify: `crates/koji-cli/src/main.rs` (if ModelCommands is defined there)

**Steps:**
- [ ] Remove `Ps` variant from `ModelCommands` enum
- [ ] Remove `ModelCommands::Ps => cmd_ps(config).await` match arm from `model::run()`
- [ ] Remove `cmd_ps` function entirely from `model.rs`
- [ ] Run `cargo build --workspace` to verify compilation
- [ ] Run `cargo test --workspace` to verify no test breakage
- [ ] Run `cargo clippy --workspace -- -D warnings` to verify no warnings
- [ ] Commit

### Task 4: Verify and clean up

**Files:**
- All modified files

**Steps:**
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Verify no unused imports remain (from removed platform/health code)
- [ ] Commit if any cleanup needed
