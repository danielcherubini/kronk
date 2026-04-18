# Koji Core Test Coverage Plan

**Goal:** Increase `koji-core` test coverage from ~30% to ~50% by adding unit and integration tests for the highest-impact untested modules.

**Architecture:** Each task adds tests against existing public APIs without modifying production code behavior. Tests use `tempfile::tempdir()` for filesystem operations, in-memory SQLite for DB tests, and mock data for external service interactions.

**Tech Stack:** Rust, `tokio`, `anyhow`, `tempfile`, `assert_matches`, `serial_test` (for shared state tests)

---

### Task 1: Proxy Forwarding Tests

**Context:**
`proxy/forward.rs` has 161 lines with 0% coverage. This module handles HTTP request forwarding in the proxy, including header manipulation, body rewriting, and streaming passthrough. The existing `proxy/forward.rs` tests in the test suite cover some streaming cases, but the core forwarding logic (header filtering, body size changes, model name rewriting) needs comprehensive coverage.

**Files:**
- Modify: `crates/koji-core/src/proxy/forward.rs` (add `#[cfg(test)]` module)

**What to implement:**
Add a `#[cfg(test)]` module with tests for:
1. `filter_request_headers()` — test that dangerous headers are stripped (`host`, `connection`, `keep-alive`, `transfer-encoding`, `upgrade`)
2. `filter_request_headers()` — test that safe headers pass through (`user-agent`, `content-type`, `authorization`)
3. `rewrite_model_name_in_body()` — test rewriting a model name in JSON body (e.g., `"model": "old-name"` → `"model": "new-name"`)
4. `rewrite_model_name_in_body()` — test when model field doesn't exist (no-op)
5. `rewrite_model_name_in_body()` — test with nested JSON structures
6. `build_forward_request()` — test building a complete forward request with method, headers, and body
7. `build_forward_request()` — test with empty body (GET request)
8. `strip_response_headers()` — test stripping hop-by-hop headers from upstream response

**Steps:**
- [ ] Read `crates/koji-core/src/proxy/forward.rs` to understand all public functions and their signatures
- [ ] Write failing test for `filter_request_headers()` stripping dangerous headers in `crates/koji-core/src/proxy/forward.rs`
- [ ] Run `cargo test --package koji-core proxy::forward::tests` — verify it fails
- [ ] Write failing test for `filter_request_headers()` passing safe headers
- [ ] Run `cargo test --package koji-core proxy::forward::tests` — verify it fails
- [ ] Write failing test for `rewrite_model_name_in_body()` with simple JSON
- [ ] Run `cargo test --package koji-core proxy::forward::tests` — verify it fails
- [ ] Write failing test for `rewrite_model_name_in_body()` when model field is absent
- [ ] Run `cargo test --package koji-core proxy::forward::tests` — verify it fails
- [ ] Write failing test for `build_forward_request()` with POST body
- [ ] Run `cargo test --package koji-core proxy::forward::tests` — verify it fails
- [ ] Write failing test for `strip_response_headers()` removing hop-by-hop headers
- [ ] Run `cargo test --package koji-core proxy::forward::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat(core): add proxy forwarding unit tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `proxy/forward.rs` coverage increases from 0% to at least 60%
- [ ] No clippy warnings introduced
- [ ] No production code behavior changes

---

### Task 2: Server Lifecycle Tests

**Context:**
`proxy/lifecycle.rs` has 153 lines with 0% coverage. This module manages the server process lifecycle — starting, stopping, restarting, and monitoring the llama.cpp server process. It handles PID tracking, graceful shutdown, and health checking.

**Files:**
- Modify: `crates/koji-core/src/proxy/lifecycle.rs` (add `#[cfg(test)]` module)

**What to implement:**
Add a `#[cfg(test)]` module with tests for:
1. `_start_server_process()` — test building the correct command line with GPU flags, context size, and model path
2. `_start_server_process()` — test that CUDA backend includes correct CUDA flags
3. `_start_server_process()` — test that CPU backend excludes GPU flags
4. `stop_server()` — test sending SIGTERM to a running process (use a dummy process)
5. `stop_server()` — test graceful shutdown timeout behavior
6. `is_server_ready()` — test health check endpoint parsing
7. `is_server_ready()` — test when server returns non-200 status
8. `_build_server_args()` — test argument assembly with various config options (port, threads, context)

**Steps:**
- [ ] Read `crates/koji-core/src/proxy/lifecycle.rs` to understand all public functions and their signatures
- [ ] Write failing test for `_build_server_args()` assembling correct command line
- [ ] Run `cargo test --package koji-core proxy::lifecycle::tests` — verify it fails
- [ ] Implement minimal test helpers (dummy process spawning)
- [ ] Run `cargo test --package koji-core proxy::lifecycle::tests` — verify failure persists
- [ ] Write failing test for stop_server() with dummy process
- [ ] Run `cargo test --package koji-core proxy::lifecycle::tests` — verify it fails
- [ ] Write failing test for is_server_ready() health check parsing
- [ ] Run `cargo test --package koji-core proxy::lifecycle::tests` — verify it fails
- [ ] Write failing test for `_build_server_args()` with CUDA backend flags
- [ ] Run `cargo test --package koji-core proxy::lifecycle::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat(core): add server lifecycle unit tests"

**Acceptance criteria:**
- [ ] All new tests pass (non-blocking — skip integration tests that require actual GPU/server)
- [ ] `proxy/lifecycle.rs` coverage increases from 0% to at least 40%
- [ ] No clippy warnings introduced
- [ ] Tests that require a real server are marked with `#[ignore]`

---

### Task 3: Model Download Tests

**Context:**
`models/download/parallel.rs` has 123 lines and `models/download/single.rs` has 62 lines — both at 0% coverage. These modules handle downloading model files from URLs, with support for parallel chunked downloads and single-file downloads. The existing `models::download` tests cover content-length parsing but not the actual download logic.

**Files:**
- Modify: `crates/koji-core/src/models/download/mod.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-core/src/models/download/parallel.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-core/src/models/download/single.rs` (add `#[cfg(test)]` module)

**What to implement:**
Add tests for:
1. `parse_content_length()` — test various Content-Length header formats (already partially covered, add more edge cases)
2. `_download_chunk()` — test chunked download with a local file server (use `tempfile`)
3. `_download_parallel()` — test parallel download of a small temp file
4. `download_single_file()` — test downloading to a temp directory
5. `resume_download()` — test resuming from an existing partial file
6. `calculate_chunk_ranges()` — test splitting a file into N chunks

**Steps:**
- [ ] Read `crates/koji-core/src/models/download/mod.rs`, `parallel.rs`, and `single.rs` to understand all public functions
- [ ] Write failing test for `calculate_chunk_ranges()` with various file sizes and chunk counts
- [ ] Run `cargo test --package koji-core models::download::tests` — verify it fails
- [ ] Write failing test for `_download_chunk()` using a local HTTP server (use `tiny_http` or similar)
- [ ] Run `cargo test --package koji-core models::download::tests` — verify it fails
- [ ] Write failing test for `download_single_file()` to temp directory
- [ ] Run `cargo test --package koji-core models::download::tests` — verify it fails
- [ ] Write failing test for `resume_download()` with partial file
- [ ] Run `cargo test --package koji-core models::download::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat(core): add model download unit tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `models/download/parallel.rs` coverage increases from 0% to at least 50%
- [ ] `models/download/single.rs` coverage increases from 0% to at least 50%
- [ ] No clippy warnings introduced

---

### Task 4: Self-Update Tests

**Context:**
`self_update.rs` has 195 lines with only 21 covered. This module handles updating the koji CLI binary itself. The existing tests cover `detect_archive_kind()` and `is_running_as_service()`, but the core update logic (`check_for_update()`, `update_binary()`, `download_and_install()`) is untested.

**Files:**
- Modify: `crates/koji-core/src/self_update.rs` (add `#[cfg(test)]` module)

**What to implement:**
Add tests for:
1. `detect_archive_kind()` — already partially covered, add more archive type edge cases (`.tar.xz`, `.zip`, plain binary)
2. `update_info()` deserialization — test parsing update info from various JSON shapes
3. `_extract_and_install()` — test extraction logic with temp archives (use `tempfile` + real tar/zip files)
4. `get_latest_version()` — test version comparison logic (semver parsing)
5. `needs_update()` — test version comparison: newer available, same version, older available

**Steps:**
- [ ] Read `crates/koji-core/src/self_update.rs` to understand all public functions and their signatures
- [ ] Write failing test for `_extract_and_install()` extracting a tar.gz archive to temp dir
- [ ] Run `cargo test --package koji-core self_update::tests` — verify it fails
- [ ] Write failing test for version comparison: newer available
- [ ] Run `cargo test --package koji-core self_update::tests` — verify it fails
- [ ] Write failing test for version comparison: same version (no update needed)
- [ ] Run `cargo test --package koji-core self_update::tests` — verify it fails
- [ ] Write failing test for `get_latest_version()` with invalid JSON response
- [ ] Run `cargo test --package koji-core self_update::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat(core): add self-update unit tests"

**Acceptance criteria:**
- [ ] All new tests pass (skip actual binary download/install tests with `#[ignore]`)
- [ ] `self_update.rs` coverage increases from ~10% to at least 35%
- [ ] No clippy warnings introduced

---

### Task 5: Update Checker Tests

**Context:**
`updates/checker.rs` has 204 lines with only 25 covered. This module checks for model and backend updates by comparing local files against remote manifests. The existing tests cover basic checking, but edge cases around manifest parsing, conflict resolution, and stale data handling are untested.

**Files:**
- Modify: `crates/koji-core/src/updates/checker.rs` (add `#[cfg(test)]` module)

**What to implement:**
Add tests for:
1. `parse_remote_manifest()` — test parsing valid manifest JSON
2. `parse_remote_manifest()` — test parsing malformed manifest (missing fields, wrong types)
3. `compare_local_vs_remote()` — test detecting new files in remote
4. `compare_local_vs_remote()` — test detecting removed files from local
5. `compare_local_vs_remote()` — test detecting modified files (hash mismatch)
6. `should_check()` — test checking interval logic (time-based)
7. `stale_data_cleanup()` — test removing entries older than retention period

**Steps:**
- [ ] Read `crates/koji-core/src/updates/checker.rs` to understand all public functions and their signatures
- [ ] Write failing test for `parse_remote_manifest()` with valid JSON
- [ ] Run `cargo test --package koji-core updates::tests` — verify it fails
- [ ] Write failing test for `compare_local_vs_remote()` detecting new files
- [ ] Run `cargo test --package koji-core updates::tests` — verify it fails
- [ ] Write failing test for `compare_local_vs_remote()` detecting modified files
- [ ] Run `cargo test --package koji-core updates::tests` — verify it fails
- [ ] Write failing test for `should_check()` with stale data
- [ ] Run `cargo test --package koji-core updates::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat(core): add update checker unit tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `updates/checker.rs` coverage increases from ~12% to at least 40%
- [ ] No clippy warnings introduced

---

### Task 6: Backend Updater & Installer Source Tests

**Context:**
`backends/updater.rs` has 60 lines with 0% coverage, and `backends/installer/source.rs` has 205 lines with only 25 covered. These modules handle backend version checking and installation source configuration (CUDA vs ROCm vs CPU). The existing tests cover some URL generation but not the update flow or source resolution logic.

**Files:**
- Modify: `crates/koji-core/src/backends/updater.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-core/src/backends/installer/source.rs` (add `#[cfg(test)]` module)

**What to implement:**
Add tests for:
1. `check_for_backend_update()` — test checking backend version against remote
2. `resolve_install_source()` — test selecting CUDA vs ROCm vs CPU based on GPU detection
3. `build_install_args()` — test building installation command arguments for different backends
4. `validate_backend_version()` — test version format validation

**Steps:**
- [ ] Read `crates/koji-core/src/backends/updater.rs` and `source.rs` to understand all public functions
- [ ] Write failing test for `resolve_install_source()` selecting CUDA backend
- [ ] Run `cargo test --package koji-core backends::tests` — verify it fails
- [ ] Write failing test for `build_install_args()` with ROCm targets
- [ ] Run `cargo test --package koji-core backends::tests` — verify it fails
- [ ] Write failing test for `validate_backend_version()` with valid/invalid versions
- [ ] Run `cargo test --package koji-core backends::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat(core): add backend updater and installer source tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `backends/updater.rs` coverage increases from 0% to at least 50%
- [ ] `backends/installer/source.rs` coverage increases from ~12% to at least 50%
- [ ] No clippy warnings introduced

---

### Task 7: Remaining Core Modules (DB Queries, Logging, GPU)

**Context:**
Several well-covered modules still have gaps. `db/queries/active_model_queries.rs` (33 lines, 5 covered), `db/queries/metrics_queries.rs` (46 lines, 15 covered), `logging.rs` (61 lines, 18 covered), and `gpu.rs` (167 lines, 112 covered) need additional edge case coverage.

**Files:**
- Modify: `crates/koji-core/src/db/queries/active_model_queries.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-core/src/db/queries/metrics_queries.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-core/src/logging.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-core/src/gpu.rs` (add `#[cfg(test)]` module)

**What to implement:**
Add tests for:
1. `get_active_models()` — test querying active models with various filter conditions
2. `insert_metric_sample()` — test inserting metrics with GPU data
3. `insert_metric_sample()` — test inserting metrics without GPU data
4. `tail_log_file()` — test tailing a log file with various line counts
5. `tail_log_file()` — test with empty file, non-existent file
6. `detect_gpu()` — test GPU detection with mock nvcc/rocminfo output
7. `suggest_context_size()` — test context size suggestions for various VRAM amounts

**Steps:**
- [ ] Read each target file to understand function signatures
- [ ] Write failing tests for active model queries with filter conditions
- [ ] Run `cargo test --package koji-core` — verify failures
- [ ] Write failing tests for metrics query insertions
- [ ] Run `cargo test --package koji-core` — verify failures
- [ ] Write failing tests for log file tailing
- [ ] Run `cargo test --package koji-core` — verify failures
- [ ] Write failing tests for GPU detection and context size suggestions
- [ ] Run `cargo test --package koji-core` — verify failures
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat(core): add edge case tests for DB queries, logging, and GPU"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] Each target file reaches at least 70% coverage
- [ ] No clippy warnings introduced

---

### Task 8: Backup Merge & Model Update Tests

**Context:**
`backup/merge.rs` has 66 lines with only 10 covered, and `models/update.rs` has 105 lines with 37 covered. These modules handle config backup merging and model update checking respectively.

**Files:**
- Modify: `crates/koji-core/src/backup/merge.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-core/src/models/update.rs` (add `#[cfg(test)]` module)

**What to implement:**
Add tests for:
1. `merge_backup_config()` — test merging configs with conflicting sections (local wins)
2. `merge_backup_config()` — test preserving backend-specific settings during merge
3. `check_model_updates()` — test detecting updated model files on disk
4. `compare_local_hash()` — test hash comparison for file integrity

**Steps:**
- [ ] Read `crates/koji-core/src/backup/merge.rs` and `models/update.rs`
- [ ] Write failing tests for backup merge conflict resolution
- [ ] Run `cargo test --package koji-core backup::merge::tests` — verify it fails
- [ ] Write failing tests for model update detection
- [ ] Run `cargo test --package koji-core models::update::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat(core): add backup merge and model update tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `backup/merge.rs` coverage increases from ~15% to at least 60%
- [ ] `models/update.rs` coverage increases from ~35% to at least 60%
- [ ] No clippy warnings introduced

---

## Expected Outcome

After all 8 tasks:
- `koji-core` coverage should reach ~50-55%
- ~2,000+ additional lines covered
- All critical paths (proxy forwarding, lifecycle, downloads, updates) have test coverage
- Existing tests continue to pass
