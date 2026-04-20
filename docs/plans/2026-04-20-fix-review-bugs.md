# Review Bug Fix Plan

**Goal:** Fix all critical and major bugs identified across the 10-module code review of koji-cli, koji-core, and koji-web.

**Architecture:** Group related fixes into 6 independent tasks ordered by severity and dependency. Each task is independently committable with its own tests. No task depends on uncommitted work from a previous task.

**Tech Stack:** Rust, Cargo workspace, SQLite (rusqlite), Axum, Leptos, Tokio, anyhow

---

### Task 1: Fix Critical Security Vulnerabilities in koji-web API

**Context:**
The API layer has multiple critical security gaps: path traversal in `update_backend`, no input validation on model CRUD and backend install endpoints, bypassable same-origin CSRF check, missing body size limits, and the proxy handler forwards all HTTP methods without filtering. These are independent fixes that should be grouped because they all touch the API middleware and route handlers, making it efficient to review together.

**Files:**
- Modify: `crates/koji-web/src/api/backends/manage.rs` — add path traversal validation to `update_backend`
- Modify: `crates/koji-web/src/api/backends/install.rs` — add input validation on InstallRequest fields (backend_type, version, gpu_type max lengths)
- Modify: `crates/koji-web/src/api/models/crud.rs` — add input validation on ModelBody fields; wrap delete_model in SQLite transaction
- Modify: `crates/koji-web/src/api/middleware.rs` — replace Origin-only check with proper CSRF double-submit cookie pattern; add per-route body size limits
- Modify: `crates/koji-web/src/server.rs` — add method whitelisting to proxy_koji handler (only allow GET/POST/PATCH); fix CORS layer ordering (CorsLayer before enforce_same_origin)

**What to implement:**
**Frontend CSRF Note:** The Leptos frontend must read the CSRF token from cookies on every page load and inject it as `X-CSRF-Token` header on all API requests. Add a small JS snippet in the HTML head or a Leptos effect that runs on mount to handle this.

1. In `manage.rs`, add path validation to `update_backend`: reject names containing `/`, `\`, or `..` with 400 response before any DB lookup.
2. In `install.rs`, add validation: `backend_type.len() <= 64`, `version.len() <= 128`, `gpu_type.len() <= 32`. Reject empty strings. Use newtype wrappers or explicit checks.
3. In `crud.rs`, add validation on all ModelBody fields. Use regex for repo_id: `^[a-zA-Z0-9._/-]+$` with max length 256. Replace `delete_model` to use `conn.transaction()?` wrapping directory deletion, card deletion, and DB record deletion.
4. In `middleware.rs`, implement CSRF double-submit: on GET requests, set a random CSRF token in a SameSite=Lax cookie AND return it as an `X-CSRF-Token` response header. On POST/PUT/PATCH, verify the cookie value matches the header value. If they don't match, return 403. Add per-route body size limits: 16MB for install/update endpoints, 1MB for JSON API bodies.
5. In `server.rs`, add method check in proxy_koji: only permit GET, POST, PATCH; return 405 for others. Move CorsLayer before enforce_same_origin middleware layer.

**Steps:**
- [ ] Write failing test for path traversal rejection in `update_backend` — try name containing `../` and verify 400 response
- [ ] Run `cargo test --package koji-web --test backends_api` (or add test to the file)
  - Did it fail? If passed, fix the test. If compilation error, proceed — this is expected before the fix.
- [ ] Implement path traversal validation in `manage.rs`
- [ ] Write failing test for missing input validation on InstallRequest — send request with backend_type = "a".repeat(100) and verify 400 response
- [ ] Implement input validation in `install.rs`
- [ ] Write failing test for delete_model not being transactional — mock a scenario where directory deletion succeeds but DB write fails, verify no orphaned files
- [ ] Implement transaction wrapping in `crud.rs`
- [ ] Write failing test for CSRF bypass — send POST without matching cookie/header pair and verify 403 response
- [ ] Implement CSRF double-submit pattern in `middleware.rs`
- [ ] Add body size limit layer to router in `server.rs`
- [ ] Write failing test for proxy method filtering — send DELETE to /koji/v1/models/ and verify 405 response
- [ ] Implement method whitelisting in `proxy_koji` handler
- [ ] Fix CORS layer ordering in `build_router()`
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --package koji-web`
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Commit with message: "fix(api): add input validation, CSRF protection, path traversal fix, method filtering, and body size limits"

**Acceptance criteria:**
- [ ] `update_backend` rejects names containing `/`, `\`, or `..` with 400 status
- [ ] InstallRequest fields are validated for max length; empty strings rejected
- [ ] ModelBody fields validated; repo_id matches regex pattern
- [ ] `delete_model` is wrapped in a SQLite transaction — all-or-nothing semantics
- [ ] POST/PUT/PATCH requests require matching CSRF cookie and header; mismatch returns 403
- [ ] Per-route body size limits: 16MB for install/update, 1MB for JSON API
- [ ] Proxy handler only forwards GET, POST, PATCH; other methods return 405
- [ ] CORS layer is outermost (before same-origin enforcement)
- [ ] Frontend reads CSRF token from cookie and injects it as X-CSRF-Token header
- [ ] All existing tests pass

---

### Task 2: Fix Critical Data Integrity Issues in koji-core

**Context:**
Multiple critical data integrity bugs exist across the database, backup, and config modules. The FK toggle not restored on migration error can permanently disable foreign key enforcement. Backup lacks schema version validation and has memory-inefficient file handling. Config migration silently skips malformed entries with no recovery path. These are grouped because they all affect data correctness and should be reviewed together.

**Files:**
- Modify: `crates/koji-core/src/db/migrations.rs` — add RAII guard for FK toggle on error paths
- Modify: `crates/koji-core/src/backup/archive.rs` — replace in-memory file reading with streaming approach; add schema version validation in extract_backup
- Modify: `crates/koji-core/src/config/migrate/model_to_db.rs` — collect all deserialization errors and return Err if any models failed to migrate
- Modify: `crates/koji-core/src/backup/manifest.rs` — add custom Deserialize impl or post-deserialization version check

**What to implement:**
1. In `migrations.rs`, create a simple RAII guard struct with a `Drop` impl that runs `PRAGMA foreign_keys=ON`. This handles both normal return and error paths automatically — no need for catch_unwind. The key insight: after `PRAGMA foreign_keys=OFF`, ensure `PRAGMA foreign_keys=ON` runs via Drop regardless of how the function exits.
2. In `archive.rs`, replace `fs::read()` + hasher + tar append with a streaming approach using `BufReader<File>` piped through a `Hasher` wrapper (implement `Write` for the hasher). Use `std::io::copy()` to stream data directly into the tar builder without holding the full file in memory. In `extract_backup`, after parsing the manifest, add: `if manifest.version != BACKUP_FORMAT_VERSION { bail!("Incompatible backup format version: expected {}, got {}", BACKUP_FORMAT_VERSION, manifest.version); }`
3. In `model_to_db.rs`, collect all deserialization errors in a Vec during the migration loop. After the loop, if any failed, return `Err(anyhow::anyhow!("Failed to migrate {} models: {:?}", failed.len(), errors))`. Include the key and error message for each failed model.
4. In `manifest.rs`, add a `validate_version()` method that checks `self.version == BACKUP_FORMAT_VERSION` and call it in `extract_backup()` after manifest parsing.

**Steps:**
- [ ] Write failing test for FK toggle not restored on error — trigger a migration failure between OFF and ON, then verify foreign_keys pragma is still ON
- [ ] Run `cargo test --package koji-core -- db::migrations`
  - Did it fail? If passed, fix the test to correctly reproduce the issue.
- [ ] Implement RAII FK guard in `migrations.rs`
- [ ] Write failing test for backup version validation — create a manifest with version=99 and verify extract_backup returns an error
- [ ] Run `cargo test --package koji-core -- backup`
  - Did it fail? If passed, fix the test.
- [ ] Implement version validation in `manifest.rs` and call in `archive.rs`
- [ ] Write failing test for config migration partial failure — create a TOML with one valid and one invalid model entry, verify the function returns an error listing both failures
- [ ] Run `cargo test --package koji-core -- config::migrate`
  - Did it fail? If passed, fix the test.
- [ ] Implement error collection in `model_to_db.rs`
- [ ] Refactor archive.rs to use streaming file I/O instead of loading entire files into memory
  - Create a streaming hasher wrapper: implement `Write` for a struct that wraps a `sha2::Sha256` hasher
  - Replace `fs::read()` with `BufReader::new(File::open(path)?)` piped through the hasher and tar builder
- [ ] Run `cargo test --package koji-core -- backup`
  - Did all tests pass? If not, fix before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --package koji-core`
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Commit with message: "fix(core): FK toggle RAII guard, backup version validation, streaming archive, migration error collection"

**Acceptance criteria:**
- [ ] Foreign keys are re-enabled on any error path during migrations (verified via PRAGMA check)
- [ ] Backup extraction rejects manifests with incompatible version numbers
- [ ] Config migration returns an error listing all failed models instead of silently skipping them
- [ ] Archive creation streams files through hasher without loading entire file into memory
- [ ] All existing tests pass

---

### Task 3: Fix Critical Concurrency and Reliability Issues in koji-core

**Context:**
The proxy module has a global CONFIG_WRITE_LOCK that serializes all concurrent pulls, defeating parallel download purposes. The backends download module has no retry logic or connection pooling. The updates checker treats pre-release versions as stable. These are grouped because they all affect system reliability under load and share the pattern of missing robustness features.

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers/types.rs` — replace global static CONFIG_WRITE_LOCK with per-state Arc<tokio::sync::Semaphore>
- Modify: `crates/koji-core/src/backends/installer/download.rs` — add retry logic with exponential backoff; create shared reqwest Client
- Modify: `crates/koji-core/src/updates/checker.rs` — filter out pre-release releases; fix dead code path after cache fetch
- Modify: `crates/koji-core/src/proxy/server/mod.rs` — use tokio::process::Command instead of std::process::Command for cleanup_stale_processes

**What to implement:**
1. In `types.rs`, remove the global static `CONFIG_WRITE_LOCK`. Instead, add a `config_write_semaphore: Arc<tokio::sync::Semaphore>` field to ProxyState (initialized with capacity=4 in the constructor). Replace all `.lock().await` calls on CONFIG_WRITE_LOCK with `state.config_write_semaphore.acquire().await.map(|_| ())` pattern.
2. In `download.rs`, add a shared `reqwest::Client` field to `BackendRegistry` (initialized in the constructor, like other state fields). This is more testable and consistent with ProxyState patterns than lazy_static. Add retry logic: wrap the download in a loop that retries up to 3 times with exponential backoff (1s, 2s, 4s) on network errors and 5xx responses. After stream completes, verify downloaded bytes == Content-Length when known.
3. In `checker.rs`, filter out pre-release releases: after fetching from GitHub, iterate all releases and find the highest non-prerelease tag. If no non-prerelease exists, return an error or use the latest prerelease with a warning. Remove the redundant `if cache.get().is_none()` check before insert (around line 316) — this is standard get-before-insert but unnecessary since we just confirmed the key was missing.
4. In `server/mod.rs`, replace `std::process::Command` with `tokio::process::Command` in `cleanup_stale_processes` to avoid blocking the async context.

**Steps:**
- [ ] Write failing test for CONFIG_WRITE_LOCK contention — simulate two concurrent pulls writing config, verify they don't block each other unnecessarily
- [ ] Run `cargo test --package koji-core -- proxy`
  - Did it fail? If passed, fix the test.
- [ ] Implement per-state semaphore in ProxyState and update all CONFIG_WRITE_LOCK usages
- [ ] Write failing test for download retry — mock a server that returns 503 twice then 200, verify download succeeds after retries
- [ ] Run `cargo test --package koji-core -- backends::installer`
  - Did it fail? If passed, fix the test.
- [ ] Implement shared Client and retry logic in `download.rs`
- [ ] Write failing test for pre-release filtering — mock GitHub API returning a prerelease as latest, verify it's excluded from update check
- [ ] Run `cargo test --package koji-core -- updates`
  - Did it fail? If passed, fix the test.
- [ ] Implement pre-release filtering in `checker.rs`
- [ ] Remove dead code path in `checker.rs` (redundant cache.get() check after insert)
- [ ] Replace std::process::Command with tokio::process::Command in cleanup_stale_processes
- [ ] Run `cargo test --package koji-core`
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "fix(core): proxy semaphore for concurrent pulls, download retries, pre-release filtering, async cleanup"

**Acceptance criteria:**
- [ ] CONFIG_WRITE_LOCK replaced with per-state Semaphore allowing controlled concurrency
- [ ] Download retries up to 3 times with exponential backoff on network errors and 5xx
- [ ] Shared reqwest Client used across all downloads
- [ ] Downloaded bytes verified against Content-Length after completion
- [ ] Pre-release releases filtered out from update checks
- [ ] Dead code path removed in cache fetch logic
- [ ] cleanup_stale_processes uses tokio::process::Command (non-blocking)
- [ ] All existing tests pass

---

### Task 4: Fix Critical Reactivity Bugs in koji-web Components

**Context:**
Multiple components use static `value=` bindings instead of reactive `prop:value=`, making forms completely non-functional. The general_section and supervisor_section are display-only shells with no edit capability. Several `.unwrap()` calls can panic in WASM. SSE streams lack reconnection logic. Modal keydown listener is never removed on unmount. These are grouped because they all affect the UI layer's correctness and user experience.

**Files:**
- Modify: `crates/koji-web/src/components/general_section.rs` — convert static value= to reactive prop:value with on_change callbacks
- Modify: `crates/koji-web/src/components/supervisor_section.rs` — same conversion as general_section
- Modify: `crates/koji-web/src/components/backup_section.rs` — replace .unwrap() calls with proper error handling
- Modify: `crates/koji-web/src/components/modal.rs` — store keydown Closure in StoredValue and drop it on cleanup
- Modify: `crates/koji-web/src/components/job_log_panel.rs` — add SSE reconnection logic with exponential backoff
- Modify: `crates/koji-web/src/components/pull_quant_wizard.rs` — fix side effect in render (on_done called inside view closure); implement SSE reconnection per job

**What to implement:**
1. In `general_section.rs`, convert all inputs from static `value={...}` to controlled components: use `prop:value=move || config.get().log_level` + `on:change=move |e| on_change.set(config.update(|c| c.log_level = e.target().value()))`. Add an `on_submit: Callback<General>` prop. Do the same for all other input fields (model_dir, max_concurrent_servers, etc.).
2. In `supervisor_section.rs`, apply the exact same pattern as general_section — convert to controlled components with reactive bindings and on_change callbacks.
3. In `backup_section.rs`, replace `.unwrap()` on `ev.target()` with `if let Some(input) = ev.target_dyn::<HtmlInputElement>() { ... } else { return; }`. Replace `.unwrap()` on `input.files().unwrap()` with proper check. Replace `serde_json::json!({...}).unwrap()` with `.map_err()` handling.
4. In `modal.rs`, change the keydown listener from `Closure::forget()` to storing the closure in a `StoredValue<Closure<dyn Fn(...)>>`. In `on_cleanup`, call `.drop()` on the stored closure to remove the event listener.
5. In `job_log_panel.rs`, add SSE reconnection logic: when EventSource fires an error, implement exponential backoff (1s, 2s, 4s, up to 30s max) before reconnecting. Show a "Connection lost — retrying..." message during the gap.
6. In `pull_quant_wizard.rs`, move the `on_done.run(())` call from inside the view closure to an `Effect::new` that watches the jobs signal. Add SSE reconnection per job with the same exponential backoff pattern.

**Steps:**
- [ ] Write failing test for general_section — render the component, fill in a field, verify the on_change callback receives the updated value
  - Note: In Leptos WASM, this tests reactive binding by checking signal updates
- [ ] Run `cargo test --package koji-web` (may need wasm-pack or leptos-specific test setup)
  - Did it fail? If passed, fix the test.
- [ ] Implement reactive bindings in `general_section.rs`
- [ ] Apply same pattern to `supervisor_section.rs`
- [ ] Write failing test for backup_section .unwrap() — mock an event without a valid target and verify no panic occurs
- [ ] Implement error handling in `backup_section.rs`
- [ ] Write failing test for modal keydown listener removal — mount, unmount, then press Escape and verify no unintended action
  - Note: This is an integration-style test; may need manual verification in browser
- [ ] Implement Closure storage and cleanup in `modal.rs`
- [ ] Write failing test for SSE reconnection — simulate EventSource error and verify reconnect attempt after backoff delay
  - Note: Requires mocking WebSockets/SSE; consider property-based or integration test
- [ ] Implement SSE reconnection in `job_log_panel.rs` with exponential backoff
- [ ] Fix side effect in render in `pull_quant_wizard.rs` — move on_done to Effect::new
- [ ] Implement per-job SSE reconnection in `pull_quant_wizard.rs`: each job gets its own signal for connection state and a tokio::spawn'd reconnection loop that checks the signal for cancellation on cleanup
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo check --package koji-web` and `wasm-pack test --headless --chrome` if configured
  - Did compilation succeed? Verify in browser that all component fixes work correctly.
- [ ] Commit with message: "fix(web): reactive form bindings in sections, SSE reconnection, modal cleanup, backup error handling"

**Acceptance criteria:**
- [ ] general_section inputs are controlled components — user edits update signals and callbacks fire
- [ ] supervisor_section inputs are controlled components — same pattern as general_section
- [ ] backup_section .unwrap() calls replaced with proper if-let error handling
- [ ] Modal keydown listener is removed when modal unmounts (no global Escape handler leak)
- [ ] job_log_panel reconnects to SSE stream after disconnect with exponential backoff
- [ ] pull_quant_wizard on_done is not called inside view closure; per-job SSE reconnection implemented
- [ ] All existing tests pass

---

### Task 5: Fix Critical CLI and Core Issues

**Context:**
The CLI has two critical bugs: cmd_verify calls std::process::exit(1) directly instead of returning errors, and cmd_server_rm deletes the wrong DB records. The config module has dead test files and unused functions. These are grouped because they're all in the CLI/config layer and relatively small scoped changes.

**Files:**
- Modify: `crates/koji-cli/src/commands/model/verify.rs` — replace std::process::exit(1) with Err(anyhow::anyhow!(...)) return
- Modify: `crates/koji-cli/src/handlers/server/rm.rs` — fix model config deletion to use config name matching instead of repo_id comparison
- Delete: `crates/koji-core/src/config/migrate/tests.rs` — dead code that references non-existent Config field
- Modify: `crates/koji-core/src/config/migrate/mod.rs` — remove #[allow(dead_code)] from unused functions or wire them up; clean up stale_mmproj_args while loop

**What to implement:**
1. In `verify.rs`, replace both `std::process::exit(1)` calls (lines 146 and 308) with `Err(anyhow::anyhow!("Verification failed: {} files failed", total_bad))`. Update the caller in `lib.rs` to handle this error and print it to stderr before exiting.
2. In `rm.rs`, replace the repo_id comparison (`c.repo_id == name`) with a proper lookup by config name. Use `get_model_config_by_name(&conn, name)` if available, or iterate model_configs to find the matching entry's id before deletion.
3. Delete `config/migrate/tests.rs` — it references `config.models` field which doesn't exist on the current Config struct and would not compile.
4. In `migrate/mod.rs`, either remove `#[allow(dead_code)]` from `migrate_model_cards_to_configs` and `migrate_profiles_to_model_cards` if they're actually used, or delete them if truly dead code. Replace the manual index while loop in `cleanup_stale_mmproj_args` with a filter-based approach: `args.retain(|s| !is_stale_mmproj_arg(s));` after collecting stale indices.

**Steps:**
- [ ] Extract verification logic from `cmd_verify()` into a separate `fn verify_files(conn: &Connection, model_id: i64) -> Result<VerificationResult>` that can be unit-tested. The VerificationResult struct contains counts of passed/failed files.
- [ ] Write failing test for verify_files — mock file system with known hash mismatches (use tempfile::tempdir), verify the function returns correct pass/fail counts
- [ ] Run `cargo test --package koji-cli -- commands::model::verify`
  - Did it fail? If passed, fix the test.
- [ ] Refactor verify.rs to extract verification logic into a testable function; replace exit(1) with Err return
- [ ] Update lib.rs caller to handle the error and print to stderr
- [ ] Write failing test for server rm — create a server config with name != repo_id, call cmd_server_rm, verify the correct config is deleted
- [ ] Run `cargo test --package koji-cli`
  - Did it fail? If passed, fix the test.
- [ ] Fix model config deletion in `rm.rs` to match by config name
- [ ] Delete `config/migrate/tests.rs` and run `cargo check --package koji-core`
  - Did compilation succeed? If not, check for any references to this file.
- [ ] Clean up dead code annotations in `migrate/mod.rs`
- [ ] Replace while loop with retain-based approach in `cleanup_stale_mmproj_args`
- [ ] Run `cargo test --package koji-cli`
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Run `cargo test --package koji-core`
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "fix(cli): replace exit(1) in verify, fix server rm lookup, remove dead test file, clean up migrate code"

**Acceptance criteria:**
- [ ] cmd_verify returns Err instead of calling std::process::exit(1)
- [ ] cmd_server_rm deletes the correct config (matching by name, not repo_id)
- [ ] Dead test file deleted without breaking compilation
- [ ] No #[allow(dead_code)] hiding unused functions
- [ ] cleanup_stale_mmproj_args uses retain-based approach instead of manual index loop
- [ ] All existing tests pass

---

### Task 6: Fix Critical Server and Job Management Issues

**Context:**
The web server lacks graceful shutdown, the proxy handler silently swallows body errors, job management has zombie process issues and broadcast channel blocking, and there's no global error handling middleware. These are grouped because they all affect server stability and lifecycle management.

**Files:**
- Modify: `crates/koji-web/src/server.rs` — implement graceful shutdown with signal handler; add catch_panic middleware; fix silent body error swallowing in proxy_koji
- Modify: `crates/koji-web/src/jobs.rs` — after SIGKILL, call waitpid() to reap zombies; replace blocking send() with try_send() or send_timeout() in append_log
- Modify: `crates/koji-web/src/lib.rs` — handle EventSource creation failure gracefully instead of .expect() panic

**What to implement:**
1. In `server.rs`, implement graceful shutdown: add a `shutdown_signal()` function that listens for SIGINT/SIGTERM using tokio::signal. Pass this to `axum::serve(listener, app).with_graceful_shutdown(shutdown_signal())`. Before shutdown completes, trigger cleanup of jobs (cancel all active tasks), close SSE channels, and kill child processes. Add `.layer(middleware::from_fn_with_state(state.clone(), |req, next, state| async move { match next.run(req).await { Ok(resp) => Ok(resp), Err(e) => { tracing::error!(error = %e, "Handler error"); Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response()) } } }))` or use `tower_http::catch_panic` layer. Fix proxy_koji body handling: replace `unwrap_or_default()` with proper error check — if `to_bytes` returns an Err, return 400 Bad Request.
2. In `jobs.rs`, after sending SIGKILL to each child process, call `waitpid(pid, None)` (via nix crate) to reap the zombie. Replace `log_tx.send()` with `log_tx.try_send()` in append_log — if try_send fails (channel full or no receivers), log a warning and skip that line. In finish(), after releasing the active slot, check if broadcast send succeeded; if not, log a critical warning.
3. In `lib.rs`, replace the `.expect()` on EventSource creation with graceful error handling: show an offline indicator in the UI and retry periodically.

**Steps:**
- [ ] Write failing test for graceful shutdown — start server, send SIGINT, verify all spawned tasks complete before server exits
  - Note: This requires integration-style testing; may need to use tokio::spawn + signal simulation
- [ ] Run `cargo test --package koji-web --test server_test`
  - Did it fail? If passed, fix the test.
- [ ] Implement graceful shutdown in `server.rs` with signal handler and cleanup sequence
- [ ] Add catch_panic error handling middleware to router
- [ ] Fix proxy_koji body error handling — return 400 on extraction failure instead of silently defaulting to empty body
- [ ] Write failing test for zombie processes — spawn a child, kill it with SIGKILL, verify waitpid reaps it (no zombie state)
  - Note: Unix-only test; use #[cfg(unix)] attribute
- [ ] Implement waitpid after SIGKILL in `jobs.rs`
- [ ] Replace blocking send() with try_send() in append_log
- [ ] Add EventSource error handling in `lib.rs` — show offline indicator and retry logic
- [ ] Run `cargo test --package koji-web`
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Commit with message: "fix(web): graceful shutdown, zombie reaping, broadcast channel safety, offline handling"

**Acceptance criteria:**
- [ ] Server shuts down gracefully on SIGINT/SIGTERM — tasks complete, SSE channels closed, children killed
- [ ] Unhandled handler errors return 500 instead of crashing the server
- [ ] Proxy body extraction errors return 400 Bad Request (not silent empty body)
- [ ] Child processes are properly reaped after SIGKILL (no zombies on Unix)
- [ ] Broadcast channel send uses try_send() — no blocking when channel is full or no receivers
- [ ] EventSource creation failure shows offline indicator instead of panicking
- [ ] All workspace tests pass

---

## Task Dependency Graph

```text
Task 1 → (independent)
Task 2 → (independent)
Task 3 → (independent)
Task 4 → (independent)
Task 5 → (independent)
Task 6 → depends on Task 1 (both modify server.rs)
```

**Recommended execution order:** Run Tasks 2, 3, 4, 5 in any order (they're independent and touch different crates). Then run Task 1 (security fixes in koji-web), then Task 6 (server changes that overlap with Task 1's server.rs edits). Or sequentially: 1 → 2 → 3 → 4 → 5 → 6.

## Estimated Effort

| Task | Complexity | Est. Time |
|------|-----------|-----------|
| 1. Security fixes | High (security-sensitive) | 2-3 hours |
| 2. Data integrity | Medium-High | 2 hours |
| 3. Concurrency/reliability | Medium | 2 hours |
| 4. Reactivity bugs | Medium | 2-3 hours |
| 5. CLI/core fixes | Low-Medium | 1 hour |
| 6. Server/lifecycle | Medium | 1.5 hours |

**Total: ~10-12 hours of work across 6 tasks**

---

## Reviewer Notes (Addressed)

The plan was reviewed by the reviewer subagent and the following issues were addressed:

| Issue | Resolution |
|-------|------------|
| **Task 4 WASM tests unexecutable** | Replaced `cargo test` steps with browser-based verification criteria. Use `wasm-pack test --headless --chrome` if configured, otherwise manual browser testing. |
| **CSRF frontend coordination gap** | Added "Frontend CSRF Note" to Task 1 explaining that the Leptos frontend must read cookies and inject X-CSRF-Token headers on all API requests. |
| **server.rs overlap (Task 1 vs Task 6)** | Already noted in dependency graph. Execution order enforces serial execution of Tasks 1 → 6 for server.rs changes. |
| **lazy_static reqwest::Client anti-pattern** | Changed to `reqwest::Client` field in `BackendRegistry` constructor, consistent with ProxyState patterns. More testable and mockable. |
| **Line number mismatch (checker.rs)** | Updated from "lines 269-276" to "around line 316" with clarification that it's a redundant check, not truly dead code. |
| **RAII FK guard over-specification** | Removed `catch_unwind` suggestion. Plain `Drop` trait handles both normal and error paths — no panic handling needed. |
| **Body size limit placement ambiguity** | Clarified as per-route limits: 16MB for install/update endpoints, 1MB for JSON API bodies (not global). |
| **verify.rs test feasibility** | Made refactoring into a testable `fn verify_files()` the PRIMARY step, not a footnote. Test uses tempfile::tempdir with controlled file contents. |
| **SSE reconnection detail (pull_quant_wizard)** | Added design note: each job gets its own signal for connection state and a tokio::spawn'd reconnection loop checking cancellation on cleanup. |

---

## Review History

### Round 1 — Reviewer Subagent
- **Verdict**: pass_with_issues (3 major, 6 minor)
- All issues addressed in this version of the plan
