# Koji Web Test Coverage Plan

**Goal:** Increase `koji-web` test coverage from ~15% to ~35% by adding tests for API routes, component logic, and server functionality. Leptos UI components are tested through their DTO/serialization contracts rather than DOM rendering.

**Architecture:** Tests use integration-style HTTP testing against the web server (similar to existing `server_test.rs`), plus unit tests on serializable types and validation functions. No DOM/JSDOM testing — focus on API contracts and data flow.

**Tech Stack:** Rust, `tokio`, `axum`, `leptos`, `reqwest`, `tempfile`

---

### Task 1: Backend API Endpoint Tests

**Context:**
`api/backends.rs` has 908 lines with 0% coverage. This is the largest untested file in the web crate. It handles all backend CRUD operations via REST API: listing, installing, removing, and activating backends. The existing `backends_api.rs` integration tests are marked `#[ignore]` and need infrastructure setup.

**Files:**
- Modify: `crates/koji-web/src/api/backends.rs` (add `#[cfg(test)]` module with unit tests)
- Create: `crates/koji-web/tests/backend_integration.rs` (new integration test file)

**What to implement:**
Unit tests for backends API:
1. `handle_list_backends()` — test returning backend list from registry
2. `handle_list_backends()` — test empty registry returns empty array
3. `handle_install_backend()` — test validation of install request body
4. `handle_install_backend()` — test accepting valid install request with progress tracking
5. `handle_remove_backend()` — test removing a backend by name/version
6. `handle_activate_backend()` — test activating a specific version
7. `backend_dto_serialization()` — test round-trip serialization of BackendCardDto

Integration tests (use existing server fixture pattern from `server_test.rs`):
8. `GET /api/backends` — test 200 response with backend list
9. `POST /api/backends/install` — test 400 on invalid JSON body
10. `DELETE /api/backends/{name}/{version}` — test 404 for unknown backend

**Steps:**
- [ ] Read `crates/koji-web/src/api/backends.rs` to understand all route handlers and their signatures
- [ ] Write failing unit tests for handler logic (serialization, validation)
- [ ] Run `cargo test --package koji-web api::backends::tests` — verify it fails
- [ ] Create `crates/koji-web/tests/backend_integration.rs` using existing server fixture pattern
- [ ] Write failing integration test for GET /api/backends
- [ ] Run `cargo test --package koji-web backend_integration` — verify it fails
- [ ] Write failing integration test for POST /api/backends/install with invalid body
- [ ] Run `cargo test --package koji-web backend_integration` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
- [ ] Commit with message: "feat(web): add backend API unit and integration tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `api/backends.rs` coverage increases from 0% to at least 25%
- [ ] New integration test file exists with at least 3 passing tests
- [ ] No clippy warnings introduced

---

### Task 2: API Updates Endpoint Tests

**Context:**
`api/updates.rs` has 274 lines with 0% coverage. This module handles model and backend update checks via the web API, including listing available updates and applying them.

**Files:**
- Modify: `crates/koji-web/src/api/updates.rs` (add `#[cfg(test)]` module)

**What to implement:**
Unit tests for updates API:
1. `handle_check_updates()` — test returning update list from checker
2. `handle_check_updates()` — test empty results when no updates available
3. `handle_apply_update()` — test validating update request body
4. `update_dto_serialization()` — test round-trip serialization of UpdateInfoDto

Integration tests:
5. `GET /api/updates` — test 200 response with update list
6. `POST /api/updates/apply` — test 400 on invalid update request

**Steps:**
- [ ] Read `crates/koji-web/src/api/updates.rs` to understand all route handlers
- [ ] Write failing unit tests for handler logic and DTO serialization
- [ ] Run `cargo test --package koji-web api::updates::tests` — verify it fails
- [ ] Write failing integration test for GET /api/updates
- [ ] Run `cargo test --package koji-web` — verify it fails
- [ ] Write failing integration test for POST /api/updates/apply with invalid body
- [ ] Run `cargo test --package koji-web` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
- [ ] Commit with message: "feat(web): add updates API unit and integration tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `api/updates.rs` coverage increases from 0% to at least 30%
- [ ] No clippy warnings introduced

---

### Task 3: Web API Core Tests

**Context:**
`api.rs` has 574 lines with 194 covered. This is the main API router that ties together all endpoint modules. The existing tests cover some routes but many handler functions and error responses are untested.

**Files:**
- Modify: `crates/koji-web/src/api.rs` (add `#[cfg(test)]` module)

**What to implement:**
Unit tests for API router:
1. `router()` — test that all expected routes are registered
2. Handler error response serialization — test consistent error format
3. Request body validation — test rejecting malformed JSON for all endpoints
4. `handle_health()` — test health check endpoint returns 200

Integration tests:
5. `GET /api/health` — test health endpoint
6. `GET /api/models` — test model listing via API router
7. `POST /api/models/pull` — test pull request validation

**Steps:**
- [ ] Read `crates/koji-web/src/api.rs` to understand all route registrations
- [ ] Write failing unit tests for route registration verification
- [ ] Run `cargo test --package koji-web api::tests` — verify it fails
- [ ] Write failing integration tests for health and model endpoints
- [ ] Run `cargo test --package koji-web` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
- [ ] Commit with message: "feat(web): add API router unit and integration tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `api.rs` coverage increases from ~34% to at least 55%
- [ ] No clippy warnings introduced

---

### Task 4: Component Validation & DTO Tests

**Context:**
Several component files have testable logic (validation, DTOs) but limited coverage. `components/form_validation.rs` (28 lines, 24 covered — already good), `components/install_modal.rs` (60 lines, 4 covered), and `components/backend_card.rs` (33 lines, 5 covered) need more serialization and validation tests.

**Files:**
- Modify: `crates/koji-web/src/components/install_modal.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-web/src/components/backend_card.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-web/src/components/sparkline.rs` (add `#[cfg(test)]` module)

**What to implement:**
1. Install modal DTO tests — test serialization of InstallRequestDto with various GPU types
2. Backend card DTO tests — test serialization with all backend types (CUDA, ROCm, Vulkan, CPU)
3. Sparkline data tests — test sparkline data point serialization and range calculation
4. Context length selector tests — test context length option serialization

**Steps:**
- [ ] Read each target file to understand struct definitions and their Serialize/Deserialize derives
- [ ] Write failing tests for InstallRequestDto serialization with all GPU types
- [ ] Run `cargo test --package koji-web components::install_modal::tests` — verify it fails
- [ ] Write failing tests for BackendCardDto with different backend types
- [ ] Run `cargo test --package koji-web components::backend_card::tests` — verify it fails
- [ ] Write failing tests for Sparkline data point serialization
- [ ] Run `cargo test --package koji-web components::sparkline::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
- [ ] Commit with message: "feat(web): add component DTO and validation tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] Each target file reaches at least 60% coverage
- [ ] No clippy warnings introduced

---

### Task 5: Jobs Module Tests

**Context:**
`jobs.rs` has 84 lines with 75 covered (~89%). This is already well-covered but has a few gaps in error handling and edge cases around job state transitions.

**Files:**
- Modify: `crates/koji-web/src/jobs.rs` (add `#[cfg(test)]` module)

**What to implement:**
Additional tests for jobs:
1. `submit_job()` — test submitting a job with duplicate ID (returns AlreadyRunning)
2. `submit_job()` — test submitting when max concurrent jobs reached
3. `finish_job()` — test finishing a job that doesn't exist (returns error)
4. `kill_children()` — test killing all child processes of a job
5. `broadcast_update()` — test broadcasting to all subscribers
6. Job state transition edge cases — test invalid transitions

**Steps:**
- [ ] Read `crates/koji-web/src/jobs.rs` to understand the JobManager implementation
- [ ] Write failing tests for duplicate job submission
- [ ] Run `cargo test --package koji-web jobs::tests` — verify it fails
- [ ] Write failing tests for finish_job() with non-existent job
- [ ] Run `cargo test --package koji-web jobs::tests` — verify it fails
- [ ] Write failing tests for broadcast to subscribers
- [ ] Run `cargo test --package koji-web jobs::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
- [ ] Commit with message: "feat(web): add jobs module edge case tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `jobs.rs` coverage increases from ~89% to at least 95%
- [ ] No clippy warnings introduced

---

### Task 6: Server & Types Tests

**Context:**
`server.rs` has 123 lines with 59 covered (~48%), and `types/config.rs` has 158 lines with 81 covered (~51%). These modules handle web server startup/configuration and config type definitions respectively.

**Files:**
- Modify: `crates/koji-web/src/server.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-web/src/types/config.rs` (add `#[cfg(test)]` module)

**What to implement:**
1. Server config tests — test loading config from various sources (env, file, defaults)
2. Server startup tests — test server binds to specified port
3. Config type serialization — test ConfigDto round-trip serialization
4. Config validation — test rejecting invalid config values (negative ports, etc.)

**Steps:**
- [ ] Read `crates/koji-web/src/server.rs` and `types/config.rs`
- [ ] Write failing tests for server config loading
- [ ] Run `cargo test --package koji-web` — verify it fails
- [ ] Write failing tests for ConfigDto round-trip serialization
- [ ] Run `cargo test --package koji-web types::config::tests` — verify it fails
- [ ] Write failing tests for config validation edge cases
- [ ] Run `cargo test --package koji-web types::config::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
- [ ] Commit with message: "feat(web): add server and config type tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] `server.rs` coverage increases from ~48% to at least 70%
- [ ] `types/config.rs` coverage increases from ~51% to at least 75%
- [ ] No clippy warnings introduced

---

### Task 7: Remaining Web API (Backup, Self-Update, Middleware)

**Context:**
`api/backup.rs` (131 lines, 0 covered), `api/self_update.rs` (104 lines, 0 covered), and `api/middleware.rs` (15 lines, 0 covered) are all at 0% coverage.

**Files:**
- Modify: `crates/koji-web/src/api/backup.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-web/src/api/self_update.rs` (add `#[cfg(test)]` module)
- Modify: `crates/koji-web/src/api/middleware.rs` (add `#[cfg(test)]` module)

**What to implement:**
1. Backup API tests — test backup creation and download endpoints
2. Self-update API tests — test self-update check and apply endpoints
3. Middleware tests — test CORS headers, request logging

**Steps:**
- [ ] Read each target file to understand handler signatures
- [ ] Write failing unit tests for backup API handlers
- [ ] Run `cargo test --package koji-web api::backup::tests` — verify it fails
- [ ] Write failing unit tests for self-update API handlers
- [ ] Run `cargo test --package koji-web api::self_update::tests` — verify it fails
- [ ] Write failing middleware tests for CORS and logging
- [ ] Run `cargo test --package koji-web api::middleware::tests` — verify it fails
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
- [ ] Commit with message: "feat(web): add backup, self-update, and middleware tests"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] Each target file reaches at least 30% coverage
- [ ] No clippy warnings introduced

---

## Expected Outcome

After all 7 tasks:
- `koji-web` coverage should reach ~35-40%
- ~1,500+ additional lines covered
- All API endpoints have test coverage (unit + integration)
- Component DTOs and validation logic are tested
- Existing tests continue to pass
