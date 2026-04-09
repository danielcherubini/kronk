# API Name for Models Plan

**Goal:** Use HuggingFace repo names (e.g. `bartowski/Qwen3-8B-GGUF`) as the model identifier in OpenAI-compatible API responses instead of internal config key slugs (e.g. `bartowski--qwen3-8b-gguf`).

**Architecture:** Add an `api_name: Option<String>` field to `ModelConfig` (replacing `display_name`). OpenAI API endpoints use `api_name` (falling back to config key) for model `"id"` in responses, and resolve incoming model names by matching against `api_name` (primary) then `model` field (fallback). The Koji management API continues using config key slugs unchanged.

**Tech Stack:** Rust, serde, TOML config, axum handlers

---

### Task 1: Rename `display_name` to `api_name` in `ModelConfig` and update all references

**Context:**
The `ModelConfig` struct has a field `display_name: Option<String>` that is currently unused (always set to `None` during pull, set from card migration but never read for API purposes). We are replacing it with `api_name: Option<String>` which will serve as the user-facing model identifier in OpenAI API responses. This is a pure rename — no behavioral changes in this task.

The field `display_name` appears in ~90 locations across the codebase, but many of those are for BACKEND display names (in `koji-web/src/api/backends.rs`, `components/backend_card.rs`, etc.) and must NOT be changed. Only MODEL-related `display_name` references should be renamed.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs` — rename field definition
- Modify: `crates/koji-core/src/config/migrate.rs` — update migration reference
- Modify: `crates/koji-core/src/config/resolve.rs` — update test literals
- Modify: `crates/koji-core/src/config/loader.rs` — update test literals
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs` — update pull setup
- Modify: `crates/koji-core/src/proxy/mod.rs` — update ModelConfig literals
- Modify: `crates/koji-core/src/proxy/status.rs` — update test helper
- Modify: `crates/koji-core/src/proxy/server/mod.rs` — update ModelConfig literals
- Modify: `crates/koji-cli/src/commands/model.rs` — update CLI model command
- Modify: `crates/koji-cli/src/handlers/server/add.rs` — update server add handler
- Modify: `crates/koji-cli/tests/tests.rs` — update CLI test
- Modify: `crates/koji-web/src/pages/model_editor.rs` — update form fields, signals, JSON
- Modify: `crates/koji-web/src/pages/config_editor.rs` — update config editor struct
- Modify: `crates/koji-web/src/api.rs` — update API types
- Modify: `crates/koji-web/src/types/config.rs` — update web config type
- Modify: `crates/koji-web/tests/server_test.rs` — update test fixtures
- Modify: `crates/koji-web/tests/config_structured_test.rs` — update TOML fixture

**What to implement:**
Rename the `display_name` field in `ModelConfig` to `api_name`. The field keeps the same type `Option<String>`, the same serde attributes (add `#[serde(alias = "display_name")]` for backwards compat with existing TOML files that may have `display_name` set, and keep `#[serde(skip_serializing_if = "Option::is_none")]`).

Every `ModelConfig` literal across the codebase that sets `display_name: None` or `display_name: Some(...)` must be updated to `api_name: None` or `api_name: Some(...)`.

DO NOT change `display_name` references that belong to backend types (e.g. `BackendConfig`, `BackendRegistryEntry`, backend card components). Those are a different field on a different struct.

**Steps:**
- [ ] In `crates/koji-core/src/config/types.rs`, rename the `display_name` field to `api_name` on the `ModelConfig` struct. Add `#[serde(alias = "display_name")]` above it for backwards compatibility. Keep `#[serde(skip_serializing_if = "Option::is_none")]`.
- [ ] Run `cargo build --workspace 2>&1 | head -50` to see ALL compilation errors from the rename. Every error points to a location that needs updating.
- [ ] Fix every compilation error by renaming `display_name` to `api_name` in each location. Use the compiler errors as your guide. Remember: do NOT change backend-related `display_name` references.
- [ ] Run `cargo build --workspace` — it must compile clean.
- [ ] Run `cargo test --workspace` — all tests must pass (behavior is unchanged, just a field rename).
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: `refactor: rename display_name to api_name in ModelConfig`

**Acceptance criteria:**
- [ ] `ModelConfig` has field `api_name: Option<String>` with `#[serde(alias = "display_name")]`
- [ ] No `display_name` references remain on `ModelConfig` anywhere in the codebase
- [ ] Backend `display_name` fields are untouched
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes

---

### Task 2: Set `api_name` during model pull and migration

**Context:**
Currently, when a model is pulled, `display_name` (now `api_name`) is set to `None`. We need to set it to the HuggingFace repo ID (e.g. `"bartowski/Qwen3-8B-GGUF"`) during pull. For existing models that were pulled before this change, the migration code in `migrate.rs` should set `api_name` from the `model` field (which already stores the HF repo ID).

This task makes `api_name` always populated for models that have a `model` field, ensuring the OpenAI API layer (Task 3) has a name to use.

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs` — set `api_name` during pull
- Modify: `crates/koji-core/src/config/migrate.rs` — update migration to derive `api_name` from `model` field

**What to implement:**

1. **Pull setup** (`_setup_model_after_pull_with_config()` in `koji_handlers.rs`, around line 1163): Change `api_name: None` to `api_name: Some(repo_id.to_string())`. The `repo_id` variable is already in scope — it's the full HF repo ID like `"bartowski/Qwen3-8B-GGUF"`.

2. **Migration** (`migrate.rs`): The current code sets `display_name` (now `api_name`) from `card.model.name` inside a `if let Some(card) = card_data.get(&filename)` block (around lines 88-92). This only runs for models that have card files. We need the `api_name` derivation to run for **ALL** models, not just those with cards.

   **IMPORTANT placement:** Add a **new, separate block** in the `for model_config in config.models.values_mut()` loop (line 44), immediately after the loop's opening brace — before the `if model_config.sampling.is_none()` check at line 46. This new block should be:
   ```rust
   // Derive api_name from model field (HF repo ID) if not set
   if model_config.api_name.is_none() {
       if let Some(repo_id) = &model_config.model {
           model_config.api_name = Some(repo_id.clone());
       }
   }
   ```
   Then **remove** the old `display_name`/`api_name` assignment from inside the card conditional block (lines 89-92 that set it from `card.model.name`).

   This ensures ALL existing models (not just those with card files) automatically get their HF repo name as the API name when config is loaded.

   **Note:** This is a deliberate semantic shift. Previously, `display_name` was set from `card.model.name` (a short human-friendly name like `"Qwen3-8B-GGUF"`). Now `api_name` is set from the `model` field (the full HF repo ID like `"bartowski/Qwen3-8B-GGUF"`). This is the desired behavior — users want HF repo names as API identifiers.

**Steps:**
- [ ] Write a unit test in `crates/koji-core/src/config/migrate.rs` (in the existing `#[cfg(test)]` module). Since `migrate_cards_to_unified_config()` requires filesystem fixtures, write a focused test: create a `Config` with a model entry having `api_name: None` and `model: Some("org/model-name".to_string())`, set up a minimal temp dir with an empty `configs/` directory and a `config.toml`, call the migration function, and assert `api_name == Some("org/model-name".to_string())`. Look at existing migration tests in the file to follow the same fixture setup patterns.
- [ ] Run `cargo test --package koji-core -- config::migrate::tests` — verify the new test FAILS (since migration doesn't set `api_name` from `model` yet).
- [ ] Update `_setup_model_after_pull_with_config()` in `koji_handlers.rs`: change `api_name: None` to `api_name: Some(repo_id.to_string())`.
- [ ] Update the migration logic in `migrate.rs` (around lines 89-91): replace the `card.model.name` based assignment with the `model` field based assignment described above.
- [ ] Run `cargo test --package koji-core` — all tests must pass including the new one.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: `feat: set api_name from HF repo ID during pull and migration`

**Acceptance criteria:**
- [ ] New models get `api_name = Some(repo_id)` during pull
- [ ] Existing models with `api_name: None` get it derived from `model` field during migration
- [ ] Test verifies migration behavior
- [ ] `cargo test --workspace` passes

---

### Task 3: Use `api_name` in OpenAI API list/get endpoints

**Context:**
The OpenAI-compatible API endpoints (`GET /v1/models` and `GET /v1/models/:model_id`) currently return the internal config key slug as the model `"id"`. After this task, they will return `api_name` (falling back to config key if `api_name` is `None`).

The Koji management API (`/koji/models/*`) is NOT changed — it continues using config key slugs. Only the OpenAI-compatible endpoints in `proxy/handlers.rs` are modified.

**Files:**
- Modify: `crates/koji-core/src/proxy/handlers.rs` — update `handle_list_models` and `handle_get_model`

**What to implement:**

1. **`handle_list_models`** (lines 258-297 in `handlers.rs`): Currently iterates `config.models` and returns `"id": config_name`. Change to return `"id": server_cfg.api_name.as_deref().unwrap_or(config_name)` instead. The variable `config_name` is the config key (slug) and `server_cfg` is the `&ModelConfig` — both already in scope in the loop.

2. **`handle_get_model`** (lines 176-228 in `handlers.rs`): This function matches a model by `model_id` path parameter. Currently, the config lock is acquired at line 201 (AFTER the first early-return block). The models runtime state is NOT read at all in this function currently.

   **Restructure the function** to acquire both locks upfront (similar to how `handle_list_models` does it):
   ```rust
   let config = state.config.read().await;
   let loaded_models = state.models.read().await;
   ```
   Place these at the top of the function, BEFORE the model state check. Then remove the separate `state.get_model_state(&model_id).await` call and instead look up runtime state directly from `loaded_models.get(&model_id)`.

   - **First check** (model state found by config key): If `loaded_models.get(&model_id)` returns `Some`, also look up `config.models.get(&model_id)` to get the `ModelConfig`, and use `server_cfg.api_name.as_deref().unwrap_or(&model_id)` as the response `"id"`.
   - **Fallback config loop**: Update the matching condition to ALSO check `server_cfg.api_name.as_deref() == Some(&*model_id)`. When returning the JSON, use `server_cfg.api_name.as_deref().unwrap_or(config_name)` as the `"id"`. **IMPORTANT:** When a match is found in the config loop, check runtime state via `loaded_models.get(config_name)` (the config key slug) to determine accurate `"ready"` status. Use `ms.is_ready()` if state is found, `false` otherwise. This ensures that a loaded model queried by `api_name` correctly reports `"ready": true`.

**Steps:**
- [ ] Read `crates/koji-core/src/proxy/handlers.rs` in full to understand current implementation.
- [ ] In `handle_list_models`, change the `"id"` value from `config_name` to `server_cfg.api_name.as_deref().unwrap_or(config_name)` (or equivalent — the exact variable names may differ; use whatever is in scope).
- [ ] In `handle_get_model`, restructure:
  - Move lock acquisition to the top: `let config = state.config.read().await;` and `let loaded_models = state.models.read().await;`. Replace the `state.get_model_state()` call with `loaded_models.get(&model_id)`.
  - First check (runtime state found by config key): also look up `config.models.get(&model_id)` and use its `api_name` for the response `"id"`.
  - Fallback loop: add `server_cfg.api_name.as_deref() == Some(&*model_id)` to the match condition. Use `api_name` for the response `"id"`. Check `loaded_models.get(config_name)` for accurate `"ready"` status.
- [ ] Run `cargo build --workspace` — must compile.
- [ ] Run `cargo test --workspace` — all tests must pass.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: `feat: return api_name as model id in OpenAI list/get endpoints`

**Acceptance criteria:**
- [ ] `GET /v1/models` returns `api_name` (e.g. `"bartowski/Qwen3-8B-GGUF"`) as model `"id"` for each model
- [ ] `GET /v1/models/:model_id` accepts `api_name` as lookup key and returns it as `"id"`
- [ ] Falls back to config key if `api_name` is `None`
- [ ] `cargo test --workspace` passes

---

### Task 4: Use `api_name` in OpenAI API model resolution (chat/completions)

**Context:**
When a user sends a chat completion request with `"model": "bartowski/Qwen3-8B-GGUF"`, the system needs to resolve that to the correct backend server. Currently, `resolve_servers_for_model()` in `config/resolve.rs` matches on `config_name == model_name || server.model == Some(model_name)`. We need to add `api_name` matching as the PRIMARY match criterion.

This is the critical change that makes the whole feature work end-to-end: users can now use the HF repo name in their API requests.

**Files:**
- Modify: `crates/koji-core/src/config/resolve.rs` — update `resolve_servers_for_model()` and `resolve_server()` to match on `api_name`
- Test: `crates/koji-core/src/config/resolve.rs` — add tests for `api_name` matching

**What to implement:**

1. **`resolve_servers_for_model()`** (lines 28-50): Update the match condition at line 44 from:
   ```rust
   if config_name == model_name || server.model.as_deref() == Some(model_name) {
   ```
   to:
   ```rust
   if server.api_name.as_deref() == Some(model_name)
       || config_name == model_name
       || server.model.as_deref() == Some(model_name)
   {
   ```
   This makes `api_name` the highest-priority match, with config key and `model` field as fallbacks.

2. **`resolve_server()`** (lines 5-26): Update the fallback search at line 14 from:
   ```rust
   self.models.values().find(|s| s.model.as_deref() == Some(name))
   ```
   to:
   ```rust
   self.models.values().find(|s| {
       s.api_name.as_deref() == Some(name) || s.model.as_deref() == Some(name)
   })
   ```

3. **Tests**: Add tests in the existing `#[cfg(test)]` module in `resolve.rs`:
   - Test that a model with `api_name: Some("my-custom-name".into())` is found when queried by `"my-custom-name"`.
   - Test that `api_name` takes priority: if config key is `"slug"` and `api_name` is `"friendly-name"`, querying `"friendly-name"` resolves correctly.
   - Test backward compat: model with `api_name: None` is still found by config key or `model` field.

**Steps:**
- [ ] Read the existing tests in `crates/koji-core/src/config/resolve.rs` to understand the test helpers and patterns used.
- [ ] Write the new tests described above. They should FAIL because `api_name` is not yet checked in resolution.
- [ ] Run `cargo test --package koji-core -- config::resolve::tests` — verify new tests fail.
- [ ] Update `resolve_servers_for_model()` to add `api_name` matching as described above.
- [ ] Update `resolve_server()` to add `api_name` matching in the fallback search as described above.
- [ ] Run `cargo test --package koji-core` — all tests must pass.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: `feat: resolve models by api_name in server resolution`

**Acceptance criteria:**
- [ ] `resolve_servers_for_model("bartowski/Qwen3-8B-GGUF")` finds a model with `api_name: Some("bartowski/Qwen3-8B-GGUF")`
- [ ] `resolve_server("bartowski/Qwen3-8B-GGUF")` finds a model with `api_name: Some("bartowski/Qwen3-8B-GGUF")`
- [ ] Config key and `model` field matching still work as fallbacks
- [ ] New tests verify all three match paths
- [ ] `cargo test --workspace` passes

---

### Task 5: Update `status.rs` to include `api_name` in status responses

**Context:**
The `collect_model_statuses()` and `build_status_response()` functions in `status.rs` build JSON responses for the status endpoint. Currently they use the config key as the model `id`. We need to include `api_name` in these responses so that clients (including the web UI) can show the friendly name. The status response is consumed by both the Koji management API and the web dashboard.

**Files:**
- Modify: `crates/koji-core/src/gpu.rs` — add `api_name` field to `ModelStatus` struct (defined at line 82)
- Modify: `crates/koji-core/src/proxy/status.rs` — populate `api_name` in status output
- Modify: `crates/koji-web/src/pages/dashboard.rs` — add `api_name` to mirror `ModelStatus` struct (line 38)

**What to implement:**

1. **`ModelStatus` struct** (`gpu.rs`, line 82): Add `pub api_name: Option<String>` field. The struct currently has `id`, `backend`, `loaded`. The new field should be added after `id`.

2. **Dashboard mirror** (`crates/koji-web/src/pages/dashboard.rs`, line 38): Add `api_name: Option<String>` field to the mirror `ModelStatus` struct. This struct must match the server-side struct exactly (the comment at line 35 says so). Add `#[serde(default)]` on the field for forward compatibility if the server hasn't been updated yet.

3. **`collect_model_statuses()`** (status.rs, lines 13-38): Set `api_name: server_cfg.api_name.clone()` when building each `ModelStatus`.

4. **`build_status_response()`** (lines 45-164): When building each model's JSON object, add an `"api_name"` key with the value from `server_cfg.api_name`. Note: the status JSON already includes `"model"` (the HF repo ID from `model_config.model`). The `api_name` will often have the same value, but keeping both is correct because: (a) `api_name` is user-customizable and may differ, (b) `model` is the canonical HF source, (c) `api_name` is what the OpenAI API uses as the `"id"`.

**Steps:**
- [ ] Read `crates/koji-core/src/gpu.rs` (around line 82) to see the `ModelStatus` struct.
- [ ] Add `pub api_name: Option<String>` field to `ModelStatus` in `gpu.rs`.
- [ ] Read `crates/koji-web/src/pages/dashboard.rs` (around line 38) to see the mirror `ModelStatus` struct.
- [ ] Add `#[serde(default)] api_name: Option<String>` to the dashboard mirror `ModelStatus` struct.
- [ ] Read `crates/koji-core/src/proxy/status.rs` to find where `ModelStatus` is constructed.
- [ ] Update `collect_model_statuses()` to populate `api_name` from `server_cfg.api_name.clone()`.
- [ ] Update `build_status_response()` to include `"api_name"` in the JSON output for each model.
- [ ] Run `cargo build --workspace` — must compile.
- [ ] Run `cargo test --workspace` — all tests must pass.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: `feat: include api_name in model status responses`

**Acceptance criteria:**
- [ ] `ModelStatus` struct has `api_name: Option<String>` field
- [ ] Status JSON includes `"api_name"` for each model
- [ ] `cargo test --workspace` passes

---

### Task 6: Update web UI to display and edit `api_name`

**Context:**
The web UI (Leptos frontend in `crates/koji-web/`) has a model editor page and a config editor page that previously showed `display_name`. These were updated to `api_name` in Task 1 (the rename). Now we need to ensure the UI meaningfully uses `api_name`:
- The model editor should show `api_name` as the model's display identifier (instead of the slug)
- The config editor should allow editing `api_name`
- The models list/dashboard should show `api_name` where available

Since this is a Leptos (Rust WASM) frontend, changes are in `.rs` files under `crates/koji-web/src/`.

**Files:**
- Modify: `crates/koji-web/src/pages/model_editor.rs` — update to use `api_name` as primary display name
- Modify: `crates/koji-web/src/pages/models.rs` (if it exists) — show `api_name` in model list
- Modify: `crates/koji-web/src/pages/dashboard.rs` (if applicable) — show `api_name` in dashboard model cards

**What to implement:**

1. **Model editor page**: Ensure the form field label says "API Name" (not "Display Name"). The signal/binding should already be `api_name` from the Task 1 rename. Verify that saving the form includes `api_name` in the JSON payload sent to the server.

2. **Models list page**: If the models list shows model identifiers, prefer showing `api_name` (falling back to the slug/config key). Look for where model names are rendered and use `api_name` if available.

3. **Dashboard**: If the dashboard shows loaded model names, use `api_name` from the status response (added in Task 5).

**Steps:**
- [ ] Read `crates/koji-web/src/pages/model_editor.rs` to understand the current form fields and how `api_name` is used.
- [ ] Read `crates/koji-web/src/pages/models.rs` (or equivalent models list page) to see how model names are displayed.
- [ ] Read `crates/koji-web/src/pages/dashboard.rs` to see if it displays model names.
- [ ] Update model editor to label the field "API Name" and ensure it's saved correctly.
- [ ] Update models list to prefer `api_name` over slug for display.
- [ ] Update dashboard to show `api_name` from status data where available.
- [ ] Run `cargo build --workspace` — must compile (note: koji-web may need wasm target or specific build command; check the Makefile or build scripts).
- [ ] Run `cargo test --workspace` — all tests must pass.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: `feat: display api_name in web UI model pages`

**Acceptance criteria:**
- [ ] Model editor shows "API Name" field with correct binding
- [ ] Models list shows `api_name` as primary identifier where available
- [ ] Dashboard uses `api_name` from status response
- [ ] `cargo build --workspace` compiles
- [ ] `cargo test --workspace` passes

---

### Task 7: Rewrite model name in chat completion response bodies

**Context:**
When a chat completion request is forwarded to the backend (e.g. llama.cpp), the backend returns a `"model"` field in its response JSON containing whatever model identifier it uses internally (often a filename path or the model's own name). After our changes, users send requests with `"model": "bartowski/Qwen3-8B-GGUF"` (the `api_name`), but the response comes back with a different model name from the backend. For a consistent OpenAI-compatible experience, the response should echo back the same model name the user sent.

The `forward_request()` function in `proxy/forward.rs` handles both streaming (SSE) and non-streaming responses. It currently passes the response body through unchanged via `Body::from_stream(response.bytes_stream())`.

**Files:**
- Modify: `crates/koji-core/src/proxy/forward.rs` — add model name rewriting
- Modify: `crates/koji-core/src/proxy/handlers.rs` — pass model name to `forward_request`

**What to implement:**

1. **Update `forward_request` signature** to accept an additional `model_name: &str` parameter — this is the model name from the user's request (the `api_name` they used).

2. **Non-streaming responses** (Content-Type is `application/json`): After receiving the full response body, parse it as JSON, replace `response["model"]` with the `model_name` parameter, re-serialize, and return.

3. **Streaming responses** (Content-Type is `text/event-stream`): Transform the byte stream. For each SSE chunk that starts with `data: ` (and is not `data: [DONE]`), parse the JSON, replace `"model"` with `model_name`, re-serialize, and emit the modified SSE line. Pass through non-data lines (comments, empty lines) unchanged.

4. **Update callers**: In `handle_chat_completions` and `handle_stream_chat_completions` (in `handlers.rs`), pass `model_name` (the value extracted from the request body) to `forward_request`. The `model_name` variable is already in scope in both functions.

**Implementation approach for streaming:**
Use a `futures::stream::StreamExt::map()` (or similar) to transform each chunk. The response from reqwest is a `bytes::Bytes` stream. Buffer SSE lines, detect `data: ` prefixed lines, parse/modify/re-emit. Non-`data:` lines and `data: [DONE]` pass through unchanged.

**Steps:**
- [ ] Read `crates/koji-core/src/proxy/forward.rs` in full to understand the current response handling.
- [ ] Read `crates/koji-core/src/proxy/handlers.rs` lines 30-100 and 103-173 to see how `forward_request` is called.
- [ ] Add a `model_name: &str` parameter to `forward_request()`.
- [ ] For non-streaming responses (check Content-Type header for `application/json`): buffer the response body, parse as `serde_json::Value`, set `response["model"] = serde_json::Value::String(model_name.to_string())`, re-serialize to bytes, and return.
- [ ] For streaming responses (Content-Type `text/event-stream`): create a stream transformer that processes each chunk, finds `data: ` lines, parses the JSON, replaces `"model"`, and re-emits. Use `futures::stream` utilities. Handle edge cases: chunks may split across SSE boundaries, so buffer partial lines.
- [ ] Update `handle_chat_completions` call at line 99: pass `model_name` to `forward_request`.
- [ ] Update `handle_stream_chat_completions` call at line 172: pass `model_name` to `forward_request`.
- [ ] Run `cargo build --workspace` — must compile.
- [ ] Run `cargo test --workspace` — all tests must pass.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: `feat: rewrite model name in chat completion response bodies`

**Acceptance criteria:**
- [ ] Non-streaming chat completion responses have `"model"` set to the user's requested model name
- [ ] Streaming (SSE) chat completion responses have `"model"` rewritten in each `data:` chunk
- [ ] `data: [DONE]` lines pass through unchanged
- [ ] Non-chat endpoints (model list, status, etc.) are unaffected
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
