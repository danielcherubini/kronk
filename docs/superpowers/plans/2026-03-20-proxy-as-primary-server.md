# Proxy as Primary Server - Implementation Plan

**Goal:** Make the proxy the primary interface for kronk. Users run `kronk serve` to start a single server on one port that automatically manages backend lifecycles. `kronk service install` installs this as a system service.

**Architecture:** The proxy (already implemented) becomes the default entry point. The CLI gains a `serve` command (replaces `proxy start`). The `service install/start/stop/remove` commands are simplified to install/manage a single "kronk" service that runs the proxy. `kronk run <server>` is kept for manual debugging of individual backends.

**Tech Stack:** Existing - Rust, Axum, Tokio, windows-service

---

### Task 1: Add `kronk serve` command

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`

**Steps:**
- [ ] Add `Serve` variant to `Commands` enum with `--host`, `--port`, `--idle-timeout` args (same as current `ProxyCommands::Start`)
- [ ] Implement `cmd_serve()` that does exactly what `cmd_proxy()` does today (build config overrides, create ProxyState, run ProxyServer)
- [ ] Route `Commands::Serve` in the main match
- [ ] Verify: `cargo check --workspace`
- [ ] Commit

---

### Task 2: Add `--proxy` flag to `service-run` (hidden SCM entrypoint)

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`
- Modify: `crates/kronk-core/src/platform/windows.rs`
- Modify: `crates/kronk-core/src/platform/linux.rs`

**Steps:**
- [ ] Add `--proxy` bool flag to the `ServiceRun` variant (mutually exclusive with `--server`)
- [ ] When `--proxy` is set, run the proxy server instead of a single backend
- [ ] Verify: `cargo check --workspace`
- [ ] Commit

---

### Task 3: Simplify `service install` to install the proxy

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`
- Modify: `crates/kronk-core/src/platform/windows.rs`
- Modify: `crates/kronk-core/src/platform/linux.rs`

The service install logic currently iterates over servers and installs one service per server. Change it to install a single "kronk" service that runs `kronk service-run --proxy`.

**Steps:**
- [ ] Add `install_proxy_service()` to `platform/windows.rs` that registers a service named "kronk" with args `service-run --proxy`
- [ ] Add `install_proxy_service()` to `platform/linux.rs` that creates a systemd unit running `kronk service-run --proxy`
- [ ] Change `ServiceCommands::Install` to no longer require a server name - it installs the proxy service
- [ ] Keep `--server <name>` as an optional flag for the legacy single-backend mode (backward compat)
- [ ] Update `ServiceCommands::Start`, `Stop`, `Remove` similarly - default to the proxy service
- [ ] Verify: `cargo check --workspace`
- [ ] Commit

---

### Task 4: Deprecate `kronk proxy` subcommand

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`

**Steps:**
- [ ] Keep `kronk proxy start` working but print a deprecation notice pointing to `kronk serve`
- [ ] Verify: `cargo check --workspace` and `cargo test --workspace`
- [ ] Commit

---

### Task 5: Update help text and documentation

**Files:**
- Modify: `README.md`
- Modify: `crates/kronk-cli/src/main.rs` (help strings)

**Steps:**
- [ ] Update CLI help strings to reflect `kronk serve` as primary
- [ ] Update README quickstart to show `kronk serve` workflow
- [ ] Verify: `cargo check --workspace`
- [ ] Commit

---

## Summary of CLI changes

| Before | After |
|--------|-------|
| `kronk proxy start` | `kronk serve` (primary) |
| `kronk service install <server>` | `kronk service install` (installs proxy) |
| `kronk service start <server>` | `kronk service start` (starts proxy service) |
| `kronk run <server>` | `kronk run <server>` (kept for debugging) |

## Execution

After approval, execute with `execute-plan` skill.
