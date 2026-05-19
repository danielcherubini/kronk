# `whatevers-hot-n-fresh` Plan

**Goal:** Add a virtual model alias `whatevers-hot-n-fresh` that routes API requests to the most-recently-accessed loaded LLM model, or loads the last-used model from DB as a fallback.

**Architecture:** The feature intercepts the `model` field in incoming API requests. When the value is the wildcard string, `ProxyState::resolve_wildcard_model()` picks the most-recently-accessed Ready or Starting LLM model (skipping TTS backends). If no model is loaded, it falls back to the `last_used_model` table in SQLite. A new DB table tracks the last-used model across restarts. Writes are throttled (only when model changes) and best-effort.

**Tech Stack:** Rust, SQLite (rusqlite), axum, tokio

---

### Task 1: Database layer — `last_used_model` table, queries, and migration

**Context:**
We need to persist the "last used model" across proxy restarts so the wildcard fallback works after a restart. This task adds the DB table, query functions, migration, and ModelManager wrapper methods. This is pure infrastructure — no proxy code touches it yet.

**Files:**
- Create: `crates/tama-core/src/db/queries/last_used_model_queries.rs`
- Modify: `crates/tama-core/src/db/queries/mod.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/migrations.rs`
- Modify: `crates/tama-core/src/models/manager.rs`

**What to implement:**

1. In `types.rs`, add a new record type:
```rust
/// The last-used LLM model record (single row, id = 1).
#[derive(Debug, Clone)]
pub struct LastUsedModelRecord {
    pub server_name: String,  // config key (HashMap key for models map)
    pub model_name: String,   // model identifier used for load_model
    pub used_at: String,      // ISO 8601 timestamp
}
```

2. In `last_used_model_queries.rs`, implement:
```rust
/// Get the last used model. Returns None if never set.
pub fn get_last_used_model(conn: &Connection) -> Result<Option<LastUsedModelRecord>>

/// Set (or replace) the last used model. Single row, id = 1.
pub fn set_last_used_model(
    conn: &Connection,
    server_name: &str,
    model_name: &str,
) -> Result<()>
```

- `set_last_used_model` uses `INSERT OR REPLACE INTO last_used_model (id, server_name, model_name, used_at) VALUES (1, ?1, ?2, strftime(...))`
- `get_last_used_model` uses `SELECT server_name, model_name, used_at FROM last_used_model WHERE id = 1`

3. In `mod.rs`, add the module:
```rust
mod last_used_model_queries;
pub use last_used_model_queries::*;
```

4. In `migrations.rs`:
- Increment `LATEST_VERSION` from 24 to 25
- Add migration 25 to create the table:
```sql
CREATE TABLE IF NOT EXISTS last_used_model (
    id INTEGER PRIMARY KEY,
    server_name TEXT NOT NULL,
    model_name TEXT NOT NULL,
    used_at TEXT NOT NULL
);
```
- Add a test: `test_migration_v25_creates_last_used_model_table` — verify table exists with correct columns

5. In `models/manager.rs`, add two wrapper methods:
```rust
/// Returns the full record so the caller can use whichever field it needs.
/// `load_model` needs the model_name (the identifier that
/// resolve_servers_for_model can match), NOT the server_name (config key).
pub fn get_last_used(&self) -> Result<Option<LastUsedModelRecord>> {
    crate::db::queries::get_last_used_model(&self.conn)
}

/// Set the last used model. Best-effort — caller should ignore errors.
pub fn set_last_used(&self, server_name: &str, model_name: &str) -> Result<()> {
    crate::db::queries::set_last_used_model(&self.conn, server_name, model_name)
}
```

**Steps:**
- [ ] Write a failing test for `get_last_used_model` on empty table in `last_used_model_queries.rs`
  - `[ ] cargo test --package tama-core last_used_model -- --nocapture`
  - Did it fail because the table doesn't exist? If not, investigate.
- [ ] Write a failing test for `set_last_used_model` + `get_last_used_model` round-trip
  - `[ ] cargo test --package tama-core last_used_model -- --nocapture`
- [ ] Implement `LastUsedModelRecord` in `types.rs`
- [ ] Implement `get_last_used_model` and `set_last_used_model` in `last_used_model_queries.rs`
- [ ] Add module to `mod.rs`
- [ ] Run `[ ] cargo test --package tama-core last_used_model`
  - Did all tests pass? If not, fix and re-run.
- [ ] Add migration 25 to `migrations.rs` (create table)
- [ ] Increment `LATEST_VERSION` to 25
- [ ] Add test `test_migration_v25_creates_last_used_model_table` in `migrations.rs`
- [ ] Add `get_last_used()` and `set_last_used()` to `ModelManager`
- [ ] Run `[ ] cargo test --package tama-core migrations::tests::test_migration_v25`
  - Did it pass? If not, fix and re-run.
- [ ] Run `[ ] cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `[ ] cargo fmt --all`
- [ ] Run `[ ] cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add last_used_model DB table and queries"

**Acceptance criteria:**
- [ ] `last_used_model` table is created by migration v25
- [ ] `get_last_used_model` returns `None` on empty table, `Some(LastUsedModelRecord)` after `set_last_used_model`
- [ ] `set_last_used_model` replaces existing row (id = 1)
- [ ] `ModelManager::get_last_used()` returns `Result<Option<LastUsedModelRecord>>` (full record, not just server_name)
- [ ] `ModelManager::set_last_used()` compiles and works
- [ ] All existing tests still pass
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 2: Proxy state — wildcard constant and `resolve_wildcard_model` method

**Context:**
This task adds the core wildcard resolution logic to `ProxyState`. The method implements the selection strategy: (1) pick most-recently-accessed Ready/Starting LLM, (2) fall back to Failed model reload, (3) fall back to DB last_used, (4) return 503. A Mutex guard prevents concurrent redundant loads.

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`

**What to implement:**

1. In `proxy/types.rs`, add the constant near the top of the file (before `ModelState`):
```rust
/// Virtual model name that routes to whatever LLM model is active.
/// If no model is loaded, loads the last-used model from DB.
pub const WILDCARD_MODEL_NAME: &str = "whatevers-hot-n-fresh";
```

2. In `proxy/state.rs`, add a Mutex field to `ProxyState` for the concurrency guard:
```rust
/// Guard to prevent concurrent wildcard resolution from triggering
/// multiple redundant loads. Only one caller proceeds to DB lookup + load.
pub wildcard_resolve_guard: Arc<tokio::sync::Mutex<()>>,
```

3. In `ProxyState::new()` (in `state.rs`), initialize the guard:
```rust
wildcard_resolve_guard: Arc::new(tokio::sync::Mutex::new(())),
```

4. In `ProxyState::shutdown()` (in `types.rs`), clear the guard (just drop — no special cleanup needed).

5. Implement `resolve_wildcard_model` on `ProxyState` (in `state.rs`):

```rust
/// Resolve the server for a "whatevers-hot-n-fresh" request.
///
/// Selection strategy (in order):
/// 1. Most-recently-accessed Ready or Starting LLM model (by last_accessed)
/// 2. Failed LLM model — extract model_name, call load_model
/// 3. Last-used model from DB — call load_model using record's model_name field
/// 4. 503 if nothing available
///
/// Uses a Mutex guard so only one concurrent caller proceeds to DB lookup + load.
/// CRITICAL: Must drop the `self.models` read lock BEFORE calling `load_model`
/// because `load_model` acquires a write lock on the same RwLock (deadlock otherwise).
pub async fn resolve_wildcard_model(&self) -> Result<String>
```

Implementation details — two-phase approach to avoid deadlock:

**Phase 1: Collect decision data under `self.models` read lock**
- Acquire `wildcard_resolve_guard` (tokio mutex) — holds throughout
- Read `self.models` (read lock)
- Filter to non-TTS models only (`!state.is_tts_backend()`)
- Among Ready/Starting: pick the one with the most recent `last_accessed()` (use `.max_by_key()`)
- If found: clone the `server_name: String`, **drop the read lock**, return server_name
- If no Ready/Starting: check for Failed non-TTS models
  - If found: clone the `model_name: String` from the Failed state
  - **Drop the read lock** (CRITICAL — load_model needs write lock)
  - Call `self.load_model(model_name, None).await`
  - If load succeeds: return server_name
  - If load fails: continue to next fallback

**Phase 2: DB fallback (no locks held)**
- Query DB for last_used: `self.model_mgr().and_then(|mgr| mgr.get_last_used().ok())`
- If found: use the record's `model_name` field (the identifier `load_model` can resolve)
  - Call `self.load_model(&record.model_name, None).await`
  - If load succeeds: return server_name
  - If load fails: continue to error
- If nothing found: `Err(anyhow::anyhow!("No model available for '{}'", WILDCARD_MODEL_NAME))`
- Drop guard on all paths (use `let _guard = ...`)

**Key points:**
- The method takes NO parameters — it's self-contained
- For the Failed fallback: extract `model_name` from the Failed state (the model identifier)
- For the DB fallback: use `record.model_name` (the identifier), NOT `record.server_name` (config key)
- `load_model` needs a model identifier that `resolve_servers_for_model` can match (api_name, config_name, or model field) — the `model_name` from the record serves this purpose

**Steps:**
- [ ] Add `WILDCARD_MODEL_NAME` constant to `proxy/types.rs`
- [ ] Add `wildcard_resolve_guard` field to `ProxyState` struct
- [ ] Initialize the guard in `ProxyState::new()`
- [ ] Write a failing test for `resolve_wildcard_model` in `state.rs` (test module):
  - Test: no models loaded, no DB → returns Err
  - `[ ] cargo test --package tama-core proxy::state::tests::test_resolve_wildcard_no_models`
  - Did it fail? If not, investigate.
- [ ] Implement `resolve_wildcard_model` method
- [ ] Write tests:
  - `test_resolve_wildcard_picks_most_recent_ready` — two models loaded, picks the one with newer `last_accessed`
  - `test_resolve_wildcard_skips_tts` — TTS backend loaded, LLM loaded, picks LLM
  - `test_resolve_wildcard_includes_starting` — model in Starting state is available
  - `test_resolve_wildcard_fallback_to_db` — no models loaded, DB has last_used → loads it
  - `test_resolve_wildcard_no_models_no_db` — returns Err
- [ ] Run `[ ] cargo test --package tama-core proxy::state`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `[ ] cargo fmt --all`
- [ ] Run `[ ] cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add resolve_wildcard_model to ProxyState"

**Acceptance criteria:**
- [ ] `WILDCARD_MODEL_NAME` constant is defined and exported
- [ ] `resolve_wildcard_model` picks most-recently-accessed Ready/Starting LLM
- [ ] TTS backends are excluded from selection
- [ ] Falls back to DB last_used when no models loaded (uses `record.model_name`, NOT `record.server_name`)
- [ ] Returns Err when nothing available
- [ ] Mutex guard prevents concurrent redundant loads
- [ ] No deadlock: `self.models` read lock is dropped BEFORE calling `load_model`
- [ ] All tests pass

---

### Task 3: Handler integration — wildcard routing and `/v1/models` entry

**Context:**
This task wires the wildcard resolution into the HTTP handlers. Three handlers need to check for the wildcard model name before normal resolution. The `/v1/models` endpoint needs to include the virtual entry with conditional `ready` status. After successful forwarding, the last_used is updated (throttled, best-effort).

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs` (re-export constant)

**What to implement:**

1. In `proxy/mod.rs`, re-export the constant:
```rust
pub use types::WILDCARD_MODEL_NAME;
```

2. In `proxy/handlers/mod.rs`:

**a) `handle_chat_completions`** — After extracting `model_name` from the request body, before the existing `get_available_server_for_model` call:

```rust
// Check for wildcard model
if model_name == crate::proxy::WILDCARD_MODEL_NAME {
    match state.resolve_wildcard_model().await {
        Ok(server_name) => {
            state.update_last_accessed(&server_name).await;
            // Update last_used in DB (best-effort, throttled)
            update_last_used_best_effort(&state, &server_name, model_name).await;
            return forward_request(&state, &server_name, &parts, &body_bytes, Some(model_name)).await;
        }
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("No model available: {}", e),
                        "type": "NoModelError"
                    }
                })),
            ).into_response();
        }
    }
}
```

**b) `handle_stream_chat_completions`** — Same wildcard check, same logic.

**c) `handle_forward_post`** — After extracting `model_name` from the body (the `Option<String>`), add wildcard check before the existing model resolution:

```rust
let server_name = if let Some(ref model) = model_name {
    if model.as_str() == crate::proxy::WILDCARD_MODEL_NAME {
        // Wildcard handling...
    } else {
        // Existing logic...
    }
} else {
    // Existing logic...
};
```

**d) `handle_list_models`** — After building the `data` vec, prepend the virtual entry:

```rust
// Check if any non-TTS model is Ready or Starting
let has_available_llm = loaded_models.iter().any(|(_, s)| {
    !s.is_tts_backend() && (s.is_ready() || matches!(s, ModelState::Starting { .. }))
});

// Prepend virtual wildcard entry
data.insert(0, serde_json::json!({
    "id": crate::proxy::WILDCARD_MODEL_NAME,
    "object": "model",
    "created": 0,
    "owned_by": "tama-proxy",
    "ready": has_available_llm
}));
```

**e) Helper function** — Add a best-effort last_used updater:

```rust
/// Update the last_used_model in DB. Best-effort — never fails the request.
/// Throttled: only writes if the server_name differs from what's stored.
async fn update_last_used_best_effort(
    state: &ProxyState,
    server_name: &str,
    model_name: &str,
) {
    let Some(mgr) = state.model_mgr() else { return };
    // Throttle: skip write if same model
    let current = mgr.get_last_used().ok().flatten();
    if current.as_deref() == Some(server_name) {
        return; // Same model, no write needed
    }
    // Best-effort write
    let _ = mgr.set_last_used(server_name, model_name);
}
```

**f) Normal model requests** — After the existing `state.update_last_accessed(&server_name).await;` line in `handle_chat_completions` and `handle_stream_chat_completions`, also call `update_last_used_best_effort`. To get the actual model name for the DB, use the `model_name` variable already extracted from the request body. The `server_name` is the config key returned by `load_model` or `get_available_server_for_model`.

**Steps:**
- [ ] Re-export `WILDCARD_MODEL_NAME` from `proxy/mod.rs`
- [ ] Write a failing test for wildcard in `handle_chat_completions`:
  - `[ ] cargo test --package tama-core proxy::handlers::tests::test_wildcard_routes_to_loaded_model`
  - Did it fail? If not, investigate.
- [ ] Add wildcard check to `handle_chat_completions`
- [ ] Add wildcard check to `handle_stream_chat_completions`
- [ ] Add wildcard check to `handle_forward_post`
- [ ] Add `update_last_used_best_effort` helper
- [ ] Add last_used update for normal model requests (after `update_last_accessed`)
- [ ] Add virtual entry to `handle_list_models` with conditional `ready`
- [ ] Write tests:
  - `test_handle_chat_completions_wildcard_routes_to_loaded` — model loaded, wildcard routes to it
  - `test_handle_chat_completions_wildcard_503_no_models` — no models, returns 503
  - `test_handle_list_models_includes_wildcard` — virtual entry present with correct ready status
  - `test_update_last_used_best_effort_throttled` — same model = no write
- [ ] Run `[ ] cargo test --package tama-core proxy::handlers`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `[ ] cargo fmt --all`
- [ ] Run `[ ] cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: integrate wildcard routing in handlers and /v1/models"

**Acceptance criteria:**
- [ ] `POST /v1/chat/completions` with `model: "whatevers-hot-n-fresh"` routes to loaded LLM
- [ ] Returns 503 when no model available
- [ ] `GET /v1/models` includes virtual entry with conditional `ready`
- [ ] Normal model requests update `last_used_model` (throttled)
- [ ] Wildcard requests update `last_used_model` (throttled)
- [ ] `handle_forward_post` handles wildcard correctly
- [ ] All tests pass

---

### Task 4: Integration tests and final verification

**Context:**
This task adds integration-level tests that verify the complete flow: DB persistence, concurrent requests, and end-to-end handler behavior. It also runs the full workspace check to ensure nothing is broken.

**Files:**
- Modify: `crates/tama-core/src/proxy/state.rs` (add integration tests)
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs` (add integration tests)

**What to implement:**

1. Integration test in `proxy/state.rs` test module:
```rust
#[tokio::test]
async fn test_wildcard_full_flow_with_db() {
    // Setup: temp dir, config, ProxyState with db_dir
    // Verify: resolve_wildcard_model returns Err (no models)
    // Setup: manually insert a last_used record
    // Verify: resolve_wildcard_model returns the server_name from DB
}
```

2. Integration test for concurrent wildcard requests:
```rust
#[tokio::test]
async fn test_wildcard_concurrent_requests() {
    // Setup: ProxyState with no models loaded, DB with last_used
    // Spawn 5 concurrent resolve_wildcard_model calls
    // Verify: only one proceeds to load path (check via mock or log)
    // Verify: all 5 get the same server_name result
}
```

3. Test for Failed state fallback:
```rust
#[tokio::test]
async fn test_wildcard_failed_model_fallback() {
    // Setup: one model in Failed state
    // Call: resolve_wildcard_model
    // Verify: attempts to load the model (will fail in test, but check it tried)
}
```

4. Full workspace check:
```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all
```

**Steps:**
- [ ] Write `test_wildcard_full_flow_with_db` test
  - `[ ] cargo test --package tama-core test_wildcard_full_flow_with_db -- --nocapture`
- [ ] Write `test_wildcard_concurrent_requests` test
  - `[ ] cargo test --package tama-core test_wildcard_concurrent_requests -- --nocapture`
- [ ] Write `test_wildcard_failed_model_fallback` test
  - `[ ] cargo test --package tama-core test_wildcard_failed_model_fallback -- --nocapture`
- [ ] Run `[ ] cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `[ ] cargo build --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `[ ] cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `[ ] cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `[ ] cargo fmt --all`
- [ ] Commit with message: "test: add integration tests for wildcard model resolution"

**Acceptance criteria:**
- [ ] Full flow test passes (DB → resolve → load)
- [ ] Concurrent requests test passes (no duplicate loads)
- [ ] Failed model fallback test passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --all` passes

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | DB layer — table, queries, migration | 5 files |
| 2 | Proxy state — constant, resolve method | 2 files |
| 3 | Handler integration — routing, /v1/models | 2 files |
| 4 | Integration tests and verification | 2 files |

**Total:** 4 tasks, ~11 files modified, 1 file created
