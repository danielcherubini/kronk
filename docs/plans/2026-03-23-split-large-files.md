# Split Large Files Into Modules - Implementation Plan

**Goal:** Reduce file sizes by splitting the 6 largest files (500+ lines) into well-organized submodules, eliminating ~4000 lines of CLI duplication, and improving navigability.
**Architecture:** Convert large flat files into directory modules with `mod.rs` re-exporting public items. Each submodule has a single responsibility. The CLI's duplicated `main.rs`/`lib.rs` (~2000 lines each, nearly identical) are consolidated so `main.rs` becomes a thin wrapper calling into `lib.rs`.
**Tech Stack:** Rust module system, `pub use` re-exports to preserve existing public API.

**Scope:** Files 500+ lines only. Leaves `gpu.rs` (305), `profiles.rs` (363), `registry.rs` (447) untouched.

---

## Task 1: Consolidate CLI `main.rs` / `lib.rs` duplication

The biggest issue: `main.rs` (1982 lines) and `lib.rs` (1989 lines) contain nearly identical code. All logic moves into `lib.rs` modules; `main.rs` becomes ~15 lines.

**Files:**
- Modify: `crates/kronk-cli/src/main.rs` (1982 → ~15 lines)
- Modify: `crates/kronk-cli/src/lib.rs` (1989 → ~100 lines, becomes module root)
- Create: `crates/kronk-cli/src/cli.rs` (~110 lines — Args, Commands, all clap enums)
- Create: `crates/kronk-cli/src/flags.rs` (~110 lines — ExtractedFlags, extract_kronk_flags)
- Create: `crates/kronk-cli/src/service.rs` (~320 lines — Windows service dispatch, win_service_main, statics)
- Create: `crates/kronk-cli/src/handlers/mod.rs` (re-exports)
- Create: `crates/kronk-cli/src/handlers/run.rs` (~70 lines — cmd_run, build_full_args)
- Create: `crates/kronk-cli/src/handlers/serve.rs` (~60 lines — cmd_serve, cmd_proxy, start_proxy_server)
- Create: `crates/kronk-cli/src/handlers/status.rs` (~120 lines — cmd_status, format_duration_secs)
- Create: `crates/kronk-cli/src/handlers/server.rs` (~250 lines — cmd_server, cmd_server_ls/add/edit/rm, resolve_backend)
- Create: `crates/kronk-cli/src/handlers/config.rs` (~25 lines — cmd_config)
- Create: `crates/kronk-cli/src/handlers/profile.rs` (~230 lines — cmd_profile)
- Create: `crates/kronk-cli/src/handlers/service_cmd.rs` (~100 lines — cmd_service, service_start/stop_inner)
- Create: `crates/kronk-cli/src/handlers/logs.rs` (~40 lines — cmd_logs)
- Test: `crates/kronk-cli/tests/tests.rs` (existing — must still pass)

**Steps:**
- [ ] Verify all existing tests pass (`cargo test --package kronk-cli`)
- [ ] Create `cli.rs` — move `Args`, `Commands`, `ProxyCommands`, `ModelCommands`, `ServerCommands`, `ProfileCommands`, `ServiceCommands`, `ConfigCommands` from lib.rs
- [ ] Create `flags.rs` — move `ExtractedFlags` struct and `extract_kronk_flags()` from lib.rs
- [ ] Create `service.rs` — move Windows service dispatch code, statics, `win_service_main()`, `service_dispatch()` from lib.rs
- [ ] Create `handlers/` directory with `mod.rs` and all handler submodules — move each `cmd_*` function from lib.rs
- [ ] Update `lib.rs` to declare and re-export all new modules, keeping `pub mod args; pub mod commands;`
- [ ] Reduce `main.rs` to: `use kronk_cli::main as cli_main; fn main() { cli_main() }` (or equivalent thin wrapper)
- [ ] Delete all duplicated code from `main.rs`
- [ ] Run `cargo test --package kronk-cli` — verify all tests pass
- [ ] Run `cargo clippy --package kronk-cli -- -D warnings` — no warnings
- [ ] Commit: `refactor: consolidate CLI main.rs/lib.rs duplication into modules`

---

## Task 2: Split `crates/kronk-core/src/config.rs` (1135 lines)

Convert `config.rs` into a `config/` directory module.

**Files:**
- Remove: `crates/kronk-core/src/config.rs`
- Create: `crates/kronk-core/src/config/mod.rs` (~30 lines — module declarations, re-exports)
- Create: `crates/kronk-core/src/config/types.rs` (~170 lines — Config, ProxyConfig, General, BackendConfig, HealthCheck, ModelConfig, Supervisor structs + their derives)
- Create: `crates/kronk-core/src/config/defaults.rs` (~90 lines — all `default_*()` serde helper functions, Default impl for ProxyConfig)
- Create: `crates/kronk-core/src/config/loader.rs` (~280 lines — Config impl: load, save, save_to, load_from, path helpers: base_dir, config_dir, config_path, profiles_dir, configs_dir, models_dir, logs_dir, with_models_dir)
- Create: `crates/kronk-core/src/config/resolve.rs` (~220 lines — Config impl: resolve_server, resolve_servers_for_model, resolve_health_url, resolve_backend_url, resolve_health_check, resolve_profile_params, build_args, build_full_args, effective_sampling, effective_sampling_with_card, proxy_url, service_name)
- Create: `crates/kronk-core/src/config/migrate.rs` (~50 lines — migrate_model_cards_to_configs_d)
- Test: existing tests remain in `config/mod.rs` or a `config/tests.rs` submodule

**Steps:**
- [ ] Verify all existing tests pass (`cargo test --package kronk-core -- config`)
- [ ] Create `config/` directory
- [ ] Create `config/types.rs` — move all struct/enum definitions
- [ ] Create `config/defaults.rs` — move all `default_*()` functions and `impl Default for ProxyConfig`
- [ ] Create `config/loader.rs` — move Config load/save/path methods and `impl Default for Config`
- [ ] Create `config/resolve.rs` — move Config resolve_*/build_args/effective_sampling methods
- [ ] Create `config/migrate.rs` — move `migrate_model_cards_to_configs_d()`
- [ ] Create `config/mod.rs` — declare submodules, `pub use` all public items to preserve API
- [ ] Delete old `config.rs`
- [ ] Run `cargo test --package kronk-core` — verify all tests pass
- [ ] Run `cargo build --workspace` — verify no breakage in kronk-cli
- [ ] Commit: `refactor: split config.rs into config/ module directory`

---

## Task 3: Split `crates/kronk-core/src/proxy.rs` (876 lines) into `proxy/` directory

Currently `proxy.rs` coexists with `proxy/server.rs` using the Rust 2018 sibling-file pattern. Convert to `proxy/mod.rs` directory-style to add more submodules.

**Files:**
- Remove: `crates/kronk-core/src/proxy.rs`
- Create: `crates/kronk-core/src/proxy/mod.rs` (~20 lines — module declarations, re-exports)
- Create: `crates/kronk-core/src/proxy/state.rs` (~250 lines — ModelState enum + impl, ProxyMetrics struct, ProxyState struct + query methods: new, is_model_loaded, get_model_state, get_backend_url, get_backend_pid, get_circuit_breaker_failures, get_available_server_for_model, update_last_accessed, get_model_card, build_status_response)
- Create: `crates/kronk-core/src/proxy/lifecycle.rs` (~300 lines — ProxyState impl: load_model, unload_model, check_idle_timeouts — the heavy async model lifecycle logic)
- Create: `crates/kronk-core/src/proxy/process.rs` (~110 lines — override_arg, is_process_alive, kill_process, force_kill_process, check_health)
- Existing: `crates/kronk-core/src/proxy/server.rs` (719 lines — stays as-is for now, split in Task 4)

**Steps:**
- [ ] Verify tests pass (`cargo test --package kronk-core -- proxy`)
- [ ] Create `proxy/mod.rs` with module declarations for `state`, `lifecycle`, `process`, `server`
- [ ] Create `proxy/state.rs` — move ModelState, ProxyMetrics, ProxyState struct + query methods
- [ ] Create `proxy/lifecycle.rs` — move ProxyState impl methods: load_model, unload_model, check_idle_timeouts
- [ ] Create `proxy/process.rs` — move utility functions (override_arg, is_process_alive, kill_process, force_kill_process, check_health)
- [ ] Add `pub use` re-exports in `proxy/mod.rs` to preserve API (`pub use state::{ModelState, ProxyMetrics, ProxyState};`)
- [ ] Note: lifecycle.rs impl block extends ProxyState defined in state.rs — this works because they're in the same crate
- [ ] Delete old `proxy.rs`
- [ ] Run `cargo test --package kronk-core` — verify all tests pass
- [ ] Run `cargo build --workspace` — verify no breakage
- [ ] Commit: `refactor: split proxy.rs into proxy/ module directory`

---

## Task 4: Split `crates/kronk-core/src/proxy/server.rs` (719 lines)

Split the Axum proxy server into server setup vs. route handlers.

**Files:**
- Modify: `crates/kronk-core/src/proxy/server.rs` (719 → ~100 lines — ProxyServer struct + impl, router construction)
- Create: `crates/kronk-core/src/proxy/handlers.rs` (~250 lines — all handle_* functions, json_error_response)
- Create: `crates/kronk-core/src/proxy/forward.rs` (~240 lines — forward_request function)
- Modify: `crates/kronk-core/src/proxy/mod.rs` (add new module declarations)

**Steps:**
- [ ] Verify tests pass (`cargo test --package kronk-core -- proxy`)
- [ ] Create `proxy/handlers.rs` — move json_error_response, handle_chat_completions, handle_stream_chat_completions, handle_get_model, handle_status, handle_health, handle_metrics, handle_list_models, handle_fallback
- [ ] Create `proxy/forward.rs` — move forward_request function
- [ ] Update `proxy/server.rs` — keep ProxyServer struct/impl, update `use` statements to reference new modules
- [ ] Update `proxy/mod.rs` — add `mod handlers; mod forward;` (private modules, used only by server.rs)
- [ ] Run `cargo test --package kronk-core` — verify all tests pass
- [ ] Run `cargo build --workspace` — verify no breakage
- [ ] Commit: `refactor: split proxy server.rs into handlers and forward modules`

---

## Task 5: Split `crates/kronk-core/src/backends/installer.rs` (667 lines)

Split the installer into logical phases: URL resolution, downloading, extraction, and installation.

**Files:**
- Modify: `crates/kronk-core/src/backends/installer.rs` (667 → ~120 lines — InstallOptions, install_backend, prepare_target_dir, install_prebuilt)
- Create: `crates/kronk-core/src/backends/download.rs` (~50 lines — download_file with progress bar)
- Create: `crates/kronk-core/src/backends/extract.rs` (~130 lines — extract_archive, find_backend_binary)
- Create: `crates/kronk-core/src/backends/source_build.rs` (~270 lines — install_from_source, cmake build logic)
- Create: `crates/kronk-core/src/backends/urls.rs` (~80 lines — get_prebuilt_url)
- Modify: `crates/kronk-core/src/backends/mod.rs` (add new module declarations)

**Steps:**
- [ ] Verify tests pass (`cargo test --package kronk-core -- backends`)
- [ ] Create `backends/urls.rs` — move `get_prebuilt_url()`
- [ ] Create `backends/download.rs` — move `download_file()`
- [ ] Create `backends/extract.rs` — move `extract_archive()`, `find_backend_binary()`
- [ ] Create `backends/source_build.rs` — move `install_from_source()`
- [ ] Update `backends/installer.rs` — keep `InstallOptions`, `install_backend`, `prepare_target_dir`, `install_prebuilt`; update `use` imports
- [ ] Update `backends/mod.rs` — add module declarations, re-export if needed
- [ ] Run `cargo test --package kronk-core` — verify all tests pass
- [ ] Run `cargo build --workspace` — verify no breakage
- [ ] Commit: `refactor: split backends/installer.rs into focused modules`

---

## Task 6: Split `crates/kronk-core/src/models/download.rs` (518 lines)

Split the download module by download strategy.

**Files:**
- Modify: `crates/kronk-core/src/models/download.rs` (518 → ~130 lines — download_chunked entry point, build_client, cleanup_temp_files, constants)
- Create: `crates/kronk-core/src/models/download_single.rs` (~135 lines — download_single function)
- Create: `crates/kronk-core/src/models/download_parallel.rs` (~210 lines — download_parallel, download_chunk_with_retry)
- Modify: `crates/kronk-core/src/models/mod.rs` (add new module declarations)

**Steps:**
- [ ] Verify tests pass (`cargo test --package kronk-core -- models::download`)
- [ ] Create `models/download_single.rs` — move `download_single()`
- [ ] Create `models/download_parallel.rs` — move `download_parallel()`, `download_chunk_with_retry()`
- [ ] Update `models/download.rs` — keep `download_chunked`, `build_client`, `cleanup_temp_files`, constants; update `use` imports to reference new modules
- [ ] Update `models/mod.rs` — add private module declarations (`mod download_single; mod download_parallel;`)
- [ ] Run `cargo test --package kronk-core` — verify all tests pass
- [ ] Run `cargo build --workspace` — verify no breakage
- [ ] Commit: `refactor: split models/download.rs by download strategy`

---

## Summary

| Task | Target File(s) | Before (lines) | After (largest file) | Key Change |
|------|----------------|-----------------|----------------------|------------|
| 1 | CLI main.rs + lib.rs | 1982 + 1989 | ~250 (handlers/server.rs) | Eliminate duplication, thin main.rs |
| 2 | config.rs | 1135 | ~280 (loader.rs) | Directory module with types/defaults/loader/resolve |
| 3 | proxy.rs | 876 | ~300 (lifecycle.rs) | Separate state/lifecycle/process utilities |
| 4 | proxy/server.rs | 719 | ~250 (handlers.rs) | Separate handlers from server setup |
| 5 | backends/installer.rs | 667 | ~270 (source_build.rs) | Split by phase: urls/download/extract/build |
| 6 | models/download.rs | 518 | ~210 (download_parallel.rs) | Split by download strategy |

**Total lines eliminated:** ~2000 (CLI duplication) + structural improvements across 6 files.

**Invariant:** Every task preserves the public API via `pub use` re-exports. No external consumers need to change their `use` paths.

**Order:** Tasks can be done in any order, but Task 1 (CLI) is the highest-impact and should go first. Tasks 3 and 4 (proxy) should be done sequentially.
