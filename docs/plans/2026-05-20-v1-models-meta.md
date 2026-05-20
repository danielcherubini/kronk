# /v1/models Meta Enrichment Plan

**Goal:** Forward `/v1/models` and `/v1/models/:id` to each backend to get full metadata (including `meta` object), then inject `ready` into the merged response.

**Architecture:** Instead of constructing model entries from config alone, query each live backend's `/v1/models` endpoint to get authoritative data (including GGUF `meta`), merge across backends, and inject `ready` based on Tama's runtime state. Unloaded models (in config but not on any backend) are still shown from config without `meta`.

**Tech Stack:** Rust, axum, reqwest (already in use)

---

### Task 1: Add helpers to query a backend's `/v1/models`

**Context:**
We need two functions: a pure JSON parser (unit-testable) and an async HTTP helper. The pure function extracts the `data` array from a `/v1/models` response body. The async helper makes the HTTP request with a 10-second timeout and delegates parsing to the pure function.

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`
- Modify: `crates/tama-core/Cargo.toml` — add `wiremock` as a dev-dependency

**What to implement:**

1. **Pure parsing function** (unit-testable, no HTTP):

```rust
/// Parse a /v1/models response body and extract the `data` array.
/// Returns empty Vec if the response is invalid or missing `data`.
fn parse_models_response(body: &[u8]) -> Vec<serde_json::Value> {
    // Parse as serde_json::Value
    // Extract "data" field as Vec<serde_json::Value>
    // Return empty Vec on any parse error or missing field
}
```

2. **Async HTTP helper** (uses real client, tested with wiremock):

```rust
/// Query a single backend's /v1/models endpoint and return the `data` array.
/// Returns an empty Vec on any error (backend down, bad response, timeout).
async fn fetch_models_from_backend(
    state: &ProxyState,
    backend_url: &str,
) -> Vec<serde_json::Value> {
    // Build the URL: {backend_url}/v1/models
    // Send GET request using state.client with 10-second timeout
    // Parse response body using parse_models_response
    // Log a warning on failure
    // Return empty Vec on any error
}
```

Key details:
- Use `state.client.get(url).timeout(Duration::from_secs(10)).send().await` — **MUST have timeout**
- `parse_models_response` is a pure function — extract `"data"` field, return empty Vec on any error
- `fetch_models_from_backend` logs a warning on failure via `warn!` macro
- Do NOT propagate errors — return empty Vec silently
- If the response body is not valid JSON, or `"data"` is missing/not an array, return empty Vec

**Steps:**
- [ ] Add `wiremock = "0.6"` to `Cargo.toml` dev-dependencies in `tama-core`
- [ ] Implement `parse_models_response` in `mod.rs`
- [ ] Write unit tests for `parse_models_response`:
  - Valid response with `data` array → returns array
  - Invalid JSON → returns empty Vec
  - Missing `data` field → returns empty Vec
  - `data` is not an array → returns empty Vec
- [ ] Run `cargo test --package tama-core parse_models_response` — all tests must pass
- [ ] Implement `fetch_models_from_backend` in `mod.rs`
- [ ] Write an integration test in `crates/tama-core/tests/` using `wiremock`:
  - Start a mock server that returns a known `/v1/models` response
  - Call `fetch_models_from_backend` and verify it returns the `data` array
  - Test timeout: mock server that never responds → returns empty Vec after 10s
- [ ] Run `cargo test --package tama-core` — all tests must pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: add helpers to fetch /v1/models from a backend"

**Acceptance criteria:**
- [ ] `parse_models_response` is a pure function, unit-testable without HTTP
- [ ] `fetch_models_from_backend` has a 10-second timeout
- [ ] Both return empty Vec on any error
- [ ] Warning logged on failure
- [ ] All existing tests pass

---

### Task 2: Rewrite `handle_list_models` to merge backend responses

**Context:**
The current `handle_list_models` constructs model entries entirely from Tama's config. This loses all `meta` data from GGUF headers. The new approach queries each live backend's `/v1/models`, merges results, and injects `ready`.

**Critical design decisions:**
- **Locks:** Snapshot backend URLs and config data under locks, then DROP locks before making HTTP requests. Never hold `state.models` read lock across I/O.
- **Backend filtering:** Only query `ModelState::Ready` backends (those with `backend_url()`). `Starting`, `Failed`, `Unloading` states are skipped — their models appear from config without `meta`.
- **ID matching:** Do NOT match by the backend's model `id` (llama.cpp uses file paths). Match by iterating `state.models` keyed by `config_name` — we know which config each backend corresponds to.
- **Concurrency:** Query all backends concurrently using `futures::future::join_all`.
- **Fallback:** If a `Ready` backend's query fails (crashed between snapshot and request), fall back to constructing the entry from config (as if unloaded). Log a warning.
- **Deduplication:** If duplicate model IDs appear across backends, keep the first occurrence. Log a warning. Do not attempt merging.
- **Config lock:** Remove the unused `state.config.read().await` — `model_configs` lock is sufficient.

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`

**What to implement:**
Rewrite `handle_list_models` with this logic:

1. Snapshot data under locks, then drop them:
```rust
let (ready_backends, all_configs): (Vec<_>, _) = {
    let models = state.models.read().await;
    let configs = state.model_configs.read().await;

    // Collect (config_name, backend_url, is_ready) for Ready backends
    let ready_backends: Vec<_> = models.iter()
        .filter_map(|(name, ms)| {
            if let ModelState::Ready { backend_url, .. } = ms {
                Some((name.clone(), backend_url.clone(), true))
            } else {
                Some((name.clone(), None, false))
            }
        })
        .collect();

    // Clone config map for use outside lock
    configs.clone()
}; // all locks dropped here
```

2. Query all Ready backends concurrently:
```rust
let futures: Vec<_> = ready_backends.iter()
    .filter_map(|(_, url, _)| url.as_ref().map(|u| fetch_models_from_backend(&state, u)))
    .collect();
let results: Vec<Vec<serde_json::Value>> = futures::future::join_all(futures).await;
```

3. Merge results and inject `ready`:
```rust
let mut data: Vec<serde_json::Value> = Vec::new();
let mut seen_ids = HashSet::new();

for entries in results {
    for mut entry in entries {
        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if seen_ids.contains(id) {
            warn!("Duplicate model id {} from backends", id);
            continue;
        }
        seen_ids.insert(id.to_string());

        // Inject ready — look up config by the backend's config_name
        // (we know the mapping from ready_backends iteration)
        entry["ready"] = serde_json::value::to_value(true).unwrap();
        data.push(entry);
    }
}
```

4. Add unloaded models (in config but not loaded on any backend):
```rust
for (config_name, server_cfg) in all_configs.iter() {
    if !server_cfg.enabled {
        continue;
    }
    let model_id = server_cfg.api_name.as_deref().unwrap_or(config_name);
    if seen_ids.contains(model_id) {
        continue; // already added from backend
    }
    data.push(serde_json::json!({
        "id": model_id,
        "object": "model",
        "created": 0,
        "owned_by": server_cfg.backend,
        "ready": false
    }));
}
```

5. Prepend wildcard entry (`*`) — same as current logic:
```rust
let has_available_llm = ready_backends.iter().any(|(_, _, ready)| *ready);
data.insert(0, serde_json::json!({
    "id": crate::proxy::WILDCARD_MODEL_NAME,
    "object": "model",
    "created": 0,
    "owned_by": "tama-proxy",
    "ready": has_available_llm
}));
```

6. Return `{"object": "list", "data": ...}`

**Steps:**
- [ ] Write a test that verifies the handler merges models from two mock backends
  - Use `wiremock` to start two mock servers returning known `/v1/models` responses
  - Verify `meta` is present in the merged response
  - Verify `ready` is injected correctly
- [ ] Write a test that verifies unloaded models are still shown from config
  - Config with enabled model, no backend → appears with `ready: false` and no `meta`
- [ ] Write a test that verifies wildcard entry is prepended
- [ ] Rewrite `handle_list_models` in `mod.rs`
- [ ] Remove the unused `state.config.read().await` lock
- [ ] Run `cargo test --package tama-core` — all tests must pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: merge /v1/models from all backends with ready injection"

**Acceptance criteria:**
- [ ] Loaded models include `meta` object from backend
- [ ] `ready` field is injected for all models
- [ ] Unloaded models are still shown (from config, no `meta`)
- [ ] Wildcard entry (`*`) is prepended
- [ ] Locks are dropped before HTTP requests (no I/O under lock)
- [ ] Backend queries are concurrent (`join_all`)
- [ ] 10-second timeout per backend
- [ ] All existing tests pass
- [ ] Response shape matches OpenAI spec: `{"object": "list", "data": [...]}`

---

### Task 3: Rewrite `handle_get_model` to fetch from backend

**Context:**
Similar to Task 2, but for a single model. If the model is loaded on a backend, query the backend's `/v1/models` to get all models and find the matching entry. If not loaded, construct from config.

**Critical design decisions:**
- **ID mismatch:** The user-provided `model_id` may be `api_name` or config name. The backend's model `id` is a file path. We can't reliably match by ID.
- **Strategy:** Query the backend's `/v1/models` (full list), take the first entry (most backends serve one model), and inject `ready`. If the backend returns multiple models, match by checking if the config's `model` field (file path) appears in the backend's response.
- **Fallback:** If backend query fails or returns empty, fall back to constructing from config (no `meta`).

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`

**What to implement:**
Rewrite `handle_get_model` with this logic:

1. Look up the model by `model_id` in config:
   - Match by config_name, api_name, or model field (current fallback logic)
   - If not found → return 404

2. Check if the config's backend is loaded and Ready:
```rust
if let Some(ms) = loaded_models.get(config_name) {
    if let ModelState::Ready { backend_url, .. } = ms {
        // Query backend's /v1/models and find matching entry
        let entries = fetch_models_from_backend(&state, backend_url).await;
        if let Some(mut entry) = find_model_in_entries(&entries, config_name) {
            entry["ready"] = serde_json::value::to_value(true).unwrap();
            return Json(entry).into_response();
        }
    }
}
```

3. `find_model_in_entries` helper:
   - If entries has exactly one model → return it
   - If multiple → try to match by config's `model` field against backend's `id` (file path)
   - If no match → return first entry (best guess)

4. Fallback: construct from config (same as current logic, no `meta`):
```rust
return Json(serde_json::json!({
    "id": model_id_val,
    "object": "model",
    "created": 0,
    "owned_by": server_cfg.backend,
    "ready": false
})).into_response();
```

**Steps:**
- [ ] Write a test that verifies the handler fetches from backend when model is loaded
  - Mock a backend response with `meta`
  - Verify `meta` is present and `ready` is injected
- [ ] Write a test that verifies fallback to config when model is not loaded
  - Verify response has no `meta` and `ready: false`
- [ ] Write a test that verifies 404 for unknown model IDs
- [ ] Rewrite `handle_get_model` in `mod.rs`
- [ ] Run `cargo test --package tama-core` — all tests must pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: fetch /v1/models/:id from backend with meta enrichment"

**Acceptance criteria:**
- [ ] Loaded models include `meta` object from backend
- [ ] `ready` field is injected
- [ ] Unloaded models fall back to config (no `meta`)
- [ ] 404 returned for unknown model IDs
- [ ] All existing tests pass

---

### Task 4: Update tests and verify

**Context:**
Ensure all existing tests pass and add integration tests for the new behavior.

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs` (tests module)
- Modify: `crates/tama-web/tests/server_test.rs` (if applicable)

**What to implement:**
- Verify the existing handler tests still pass (they may need updates since the response shape changed)
- Add integration tests for the new behavior using `wiremock`
- Verify `meta` is present when a backend is available
- Verify `ready` is injected correctly

**Steps:**
- [ ] Run `cargo test --workspace --features web-ui` — all tests must pass
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "test: update tests for /v1/models meta enrichment"

**Acceptance criteria:**
- [ ] All workspace tests pass
- [ ] No clippy warnings
- [ ] Code is formatted

---

## Verification

After all tasks:

```bash
cargo test --workspace --features web-ui
cargo clippy --workspace -- -D warnings
cargo fmt --all
```

Manual verification:
1. Start proxy with a loaded backend
2. `curl http://localhost:11434/v1/models` — verify `meta` is present for loaded models
3. `curl http://localhost:11434/v1/models/{id}` — verify `meta` and `ready` are present
4. Verify unloaded models are still listed (without `meta`)
5. Verify wildcard entry (`*`) is prepended
