# Koji CLI Test Coverage Plan

**Goal:** Increase `koji-cli` test coverage from ~25% to ~45% by adding tests for command handlers, argument parsing, and server management logic. TUI-rendering functions are tested through their data transformation outputs rather than terminal output.

**Architecture:** Tests focus on pure function logic within handlers (arg parsing, data formatting, command validation) and integration tests using the CLI's command structure. TUI display functions are tested by verifying their input/output contracts.

**Tech Stack:** Rust, `clap`, `tokio`, `crossterm`, `inquire`, `tempfile`, `assert_cmd` (for CLI binary testing)

---

### Task 1: Backend Command Tests

**Context:**
`commands/backend.rs` has 320 lines with 0% coverage. This is the largest untested file in the CLI crate. It handles all backend-related subcommands: `koji backend list`, `koji backend install`, `koji backend remove`, `koji backend activate`, and `koji backend update`.

**Files:**
- Create: `crates/koji-cli/tests/backend_command_tests.rs` (new integration test file)

**What to implement:**
Integration tests using the CLI binary:
1. `backend list` — test listing backends with no registry (empty output)
2. `backend list` — test listing backends with installed backends
3. `backend install` — test validation: missing required arguments returns error
4. `backend install` — test accepting valid install request with progress output
5. `backend remove` — test removing a backend by name/version
6. `backend activate` — test activating a specific version
7. `backend update` — test checking for updates

**Steps:**
- [ ] Read `crates/koji-cli/src/commands/backend.rs` to understand all subcommand handlers
- [ ] Create `crates/koji-cli/tests/backend_command_tests.rs` using `assert_cmd` pattern
- [ ] Write failing test for `backend list` with empty registry
- [ ] Run `cargo test --package koji backend_command_tests` — verify it fails
- [ ] Write failing test for `backend install` with missing arguments
- [ ] Run `cargo test --package koji backend_command_tests` — verify it fails
- [ ] Write failing test for `backend remove` with valid backend
- [ ] Run `cargo test --package koji backend_command_tests` — verify it fails
- [ ] Write failing test for `backend activate` with version selection
- [ ] Run `cargo test --package koji backend_command_tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji -- -D warnings`
- [ ] Commit with message: "feat(cli): add backend command integration tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `commands/backend.rs` coverage increases from 0% to at least 40%
- [ ] New integration test file exists with at least 5 passing tests
- [ ] No clippy warnings introduced

---

### Task 2: Model Command Tests

**Context:**
`commands/model.rs` has 853 lines with 87 covered (~10%). This handles model-related subcommands: `koji model list`, `koji model pull`, `koji model remove`, `koji model verify`, `koji model scan`, and `koji model rename`. Most of the command logic is untested.

**Files:**
- Create: `crates/koji-cli/tests/model_command_tests.rs` (new integration test file)

**What to implement:**
Integration tests for model commands:
1. `model list` — test listing models with no models (empty output)
2. `model list` — test listing models with various states (loaded, idle, pulling)
3. `model remove` — test removing a model that doesn't exist (error)
4. `model verify` — test verifying a model file's hash
5. `model scan` — test scanning for new model files in a directory
6. `model rename` — test renaming a model with valid/new name
7. `model rename` — test renaming with duplicate name (error)

**Steps:**
- [ ] Read `crates/koji-cli/src/commands/model.rs` to understand all subcommand handlers
- [ ] Create `crates/koji-cli/tests/model_command_tests.rs` using `assert_cmd` pattern
- [ ] Write failing test for `model list` with empty models directory
- [ ] Run `cargo test --package koji model_command_tests` — verify it fails
- [ ] Write failing test for `model remove` with non-existent model
- [ ] Run `cargo test --package koji model_command_tests` — verify it fails
- [ ] Write failing test for `model verify` with known hash file
- [ ] Run `cargo test --package koji model_command_tests` — verify it fails
- [ ] Write failing test for `model scan` detecting new files
- [ ] Run `cargo test --package koji model_command_tests` — verify it fails
- [ ] Write failing test for `model rename` with duplicate name
- [ ] Run `cargo test --package koji model_command_tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji -- -D warnings`
- [ ] Commit with message: "feat(cli): add model command integration tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `commands/model.rs` coverage increases from ~10% to at least 35%
- [ ] New integration test file exists with at least 6 passing tests
- [ ] No clippy warnings introduced

---

### Task 3: Server Handler Tests

**Context:**
The `handlers/server/` directory has 273 lines total with only 53 covered (~19%). This includes `add.rs` (79 lines, 16 covered), `edit.rs` (49 lines, 37 covered), `ls.rs` (43 lines, 0 covered), `rm.rs` (23 lines, 0 covered), and `mod.rs` (49 lines, 27 covered). These handle the `server add/edit/ls/rm` CLI commands.

**Files:**
- Modify: `crates/koji-cli/src/handlers/server/add.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-cli/src/handlers/server/ls.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-cli/src/handlers/server/rm.rs` (add `#[cfg(test)]` module)
- Create: `crates/koji-cli/tests/server_command_tests.rs` (new integration test file)

**What to implement:**
Unit tests for server handlers:
1. `handle_server_add()` — test validating server config (required fields)
2. `handle_server_add()` — test accepting valid server configuration
3. `handle_server_ls()` — test listing servers from config
4. `handle_server_ls()` — test empty list when no servers configured
5. `handle_server_rm()` — test removing a server by name
6. `handle_server_rm()` — test removing non-existent server (error)

Integration tests:
7. `server add` — test adding a valid server configuration
8. `server ls` — test listing configured servers
9. `server rm` — test removing a configured server

**Steps:**
- [ ] Read all files in `crates/koji-cli/src/handlers/server/` to understand handler signatures
- [ ] Write failing unit tests for server add validation
- [ ] Run `cargo test --package koji handlers::server::add::tests` — verify it fails
- [ ] Write failing unit tests for server ls listing logic
- [ ] Run `cargo test --package koji handlers::server::ls::tests` — verify it fails
- [ ] Write failing unit tests for server rm removal logic
- [ ] Run `cargo test --package koji handlers::server::rm::tests` — verify it fails
- [ ] Create integration tests for server commands
- [ ] Run `cargo test --package koji server_command_tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji -- -D warnings`
- [ ] Commit with message: "feat(cli): add server handler unit and integration tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `handlers/server/add.rs` coverage increases from ~20% to at least 60%
- [ ] `handlers/server/ls.rs` coverage increases from 0% to at least 60%
- [ ] `handlers/server/rm.rs` coverage increases from 0% to at least 70%
- [ ] No clippy warnings introduced

---

### Task 4: Status Handler Tests

**Context:**
`handlers/status.rs` has 139 lines with 0% coverage. This handler displays the current status of models, backends, and the proxy server. It includes logic for formatting status output, detecting server states, and rendering progress indicators.

**Files:**
- Modify: `crates/koji-cli/src/handlers/status.rs` (add `#[cfg(test)]` module)

**What to implement:**
Unit tests for status handler:
1. `format_model_status()` — test formatting loaded model status
2. `format_model_status()` — test formatting idle model status
3. `format_model_status()` — test formatting pulling/downloading status
4. `format_backend_status()` — test formatting backend with/without active version
5. `build_status_table()` — test building a combined status table
6. `detect_server_state()` — test detecting running, stopped, and error states

**Steps:**
- [ ] Read `crates/koji-cli/src/handlers/status.rs` to understand all functions
- [ ] Write failing tests for model status formatting with various states
- [ ] Run `cargo test --package koji handlers::status::tests` — verify it fails
- [ ] Write failing tests for backend status formatting
- [ ] Run `cargo test --package koji handlers::status::tests` — verify it fails
- [ ] Write failing tests for server state detection
- [ ] Run `cargo test --package koji handlers::status::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji -- -D warnings`
- [ ] Commit with message: "feat(cli): add status handler unit tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `handlers/status.rs` coverage increases from 0% to at least 50%
- [ ] No clippy warnings introduced

---

### Task 5: Profile & Run Handler Tests

**Context:**
`handlers/profile.rs` has 57 lines with 0% coverage, and `handlers/run.rs` has 43 lines with 0% coverage. These handle profile management (list, create, edit) and the run command for starting model inference.

**Files:**
- Modify: `crates/koji-cli/src/handlers/profile.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-cli/src/handlers/run.rs` (add `#[cfg(test)]` module)

**What to implement:**
Unit tests for profile handler:
1. `list_profiles()` — test listing available profiles from config
2. `create_profile()` — test creating a new profile with valid args
3. `edit_profile()` — test editing an existing profile
4. `delete_profile()` — test deleting a profile that exists

Unit tests for run handler:
5. `build_run_args()` — test building inference arguments from config
6. `build_run_args()` — test overriding args via CLI flags
7. `validate_model_for_run()` — test validating model exists and is ready

**Steps:**
- [ ] Read `crates/koji-cli/src/handlers/profile.rs` and `run.rs` to understand function signatures
- [ ] Write failing tests for profile CRUD operations
- [ ] Run `cargo test --package koji handlers::profile::tests` — verify it fails
- [ ] Write failing tests for run argument building
- [ ] Run `cargo test --package koji handlers::run::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji -- -D warnings`
- [ ] Commit with message: "feat(cli): add profile and run handler unit tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `handlers/profile.rs` coverage increases from 0% to at least 60%
- [ ] `handlers/run.rs` coverage increases from 0% to at least 50%
- [ ] No clippy warnings introduced

---

### Task 6: Bench Handler & Lib Module Tests

**Context:**
`handlers/bench.rs` has 33 lines with 9 covered (~27%), and `lib.rs` has 47 lines with 0% coverage. The bench handler tests are already partially covered; we need to fill in the gaps. The lib module exports public API that needs documentation and test coverage.

**Files:**
- Modify: `crates/koji-cli/src/handlers/bench.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-cli/src/lib.rs` (add `#[cfg(test)]` module)

**What to implement:**
Unit tests for bench handler:
1. `parse_comma_separated_sizes()` — test parsing various size formats (already partially covered, add edge cases)
2. `build_bench_command()` — test building benchmark command with GPU flags
3. `format_bench_results()` — test formatting benchmark output

Unit tests for lib module:
4. `init_app()` — test initializing the application with default config
5. `get_config_path()` — test config path resolution

**Steps:**
- [ ] Read `crates/koji-cli/src/handlers/bench.rs` and `lib.rs`
- [ ] Write failing tests for bench command building
- [ ] Run `cargo test --package koji handlers::bench::tests` — verify it fails
- [ ] Write failing tests for lib module initialization
- [ ] Run `cargo test --package koji lib::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji -- -D warnings`
- [ ] Commit with message: "feat(cli): add bench handler and lib module tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `handlers/bench.rs` coverage increases from ~27% to at least 80%
- [ ] `lib.rs` coverage increases from 0% to at least 60%
- [ ] No clippy warnings introduced

---

### Task 7: Remaining CLI Handlers (Config, Logs, Service, Web)

**Context:**
Several small handler files are at 0% coverage: `handlers/config.rs` (12 lines), `handlers/logs.rs` (23 lines), `handlers/service_cmd.rs` (65 lines), and `handlers/web.rs` (12 lines). Also `handlers/self_update.rs` (21 lines) and `handlers/serve.rs` (61 lines).

**Files:**
- Modify: `crates/koji-cli/src/handlers/config.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-cli/src/handlers/logs.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-cli/src/handlers/service_cmd.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-cli/src/handlers/web.rs` (add `#[cfg(test)]` module)

**What to implement:**
1. Config handler tests — test config reading/writing via CLI
2. Logs handler tests — test log file tailing and filtering
3. Service command tests — test service start/stop/status commands
4. Web handler tests — test opening the web UI

**Steps:**
- [ ] Read each target file to understand handler signatures
- [ ] Write failing tests for config handler operations
- [ ] Run `cargo test --package koji handlers::config::tests` — verify it fails
- [ ] Write failing tests for log tailing/filtering
- [ ] Run `cargo test --package koji handlers::logs::tests` — verify it fails
- [ ] Write failing tests for service command operations
- [ ] Run `cargo test --package koji handlers::service_cmd::tests` — verify it fails
- [ ] Write failing tests for web handler URL opening
- [ ] Run `cargo test --package koji handlers::web::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji -- -D warnings`
- [ ] Commit with message: "feat(cli): add remaining handler unit tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] Each target file reaches at least 40% coverage
- [ ] No clippy warnings introduced

---

## Expected Outcome

After all 7 tasks:
- `koji-cli` coverage should reach ~45-50%
- ~800+ additional lines covered
- All command handlers have test coverage (unit + integration)
- TUI-related functions tested through their data contracts
- Existing tests continue to pass
