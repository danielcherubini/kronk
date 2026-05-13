# Per-Variant Default Args Plan

**Goal:** Support independent `default_args` per GPU variant (e.g., `llama_cpp:vulkan` vs `llama_cpp:rocm`) instead of sharing args across all variants of the same backend type.

**Architecture:** Add `variant_default_args: BTreeMap<String, Vec<String>>` to `BackendConfig` keyed by GPU variant name. When building args, check variant-specific args first, fall back to the shared `default_args`. The API accepts `gpu_variant` query param on the default-args endpoint.

**Tech Stack:** Rust, TOML config, Axum API, Leptos frontend (already ready — keys edits by "type:variant")

---

### Task 1: Add variant_default_args to BackendConfig

**Context:**
The `BackendConfig` struct currently stores `default_args: Vec<String>` which is shared across all GPU variants of a backend type. We need to add per-variant storage while keeping backward compatibility with existing configs that only have `default_args`.

**Files:**
- Modify: `crates/tama-core/src/config/types.rs`
- Modify: `crates/tama-core/src/config/resolve/mod.rs`
- Test: `crates/tama-core/src/config/resolve/tests/args_building.rs` (existing tests)

**What to implement:**
1. In `BackendConfig` struct (types.rs), add:
   ```rust
   /// Per-variant default args. Keyed by GPU variant name (e.g. "vulkan", "rocm", "cpu").
   /// Takes priority over the shared `default_args` when building args for a specific variant.
   #[serde(default)]
   pub variant_default_args: BTreeMap<String, Vec<String>>,
   ```
   Note: `BTreeMap` is already imported in types.rs via `use std::collections::{BTreeMap, HashMap};`.

2. In `build_args` and `build_full_args` (resolve/mod.rs), replace:
   ```rust
   let mut grouped = crate::config::merge_args(&backend.default_args, &server.args);
   ```
   With logic that checks variant_default_args first:
   ```rust
   // Use variant-specific args if available, fall back to shared default_args
   let variant_args = server
       .gpu_variant
       .as_ref()
       .and_then(|gv| backend.variant_default_args.get(gv))
       .unwrap_or(&backend.default_args);
   let mut grouped = crate::config::merge_args(variant_args, &server.args);
   ```

**Steps:**
- [ ] Add `variant_default_args: BTreeMap<String, Vec<String>>` field to `BackendConfig` in `crates/tama-core/src/config/types.rs`
- [ ] Update `build_args` in `crates/tama-core/src/config/resolve/mod.rs` to check variant_default_args (search for `merge_args(&backend.default_args` — there are 2 occurrences)
- [ ] Update `build_full_args` similarly (same pattern)
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix failures (existing tests use empty default_args, should still pass)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: add variant_default_args to BackendConfig"

**Acceptance criteria:**
- [ ] `BackendConfig` has `variant_default_args: BTreeMap<String, Vec<String>>` field with `#[serde(default)]`
- [ ] `build_full_args` checks `variant_default_args[gpu_variant]` first, falls back to `default_args`
- [ ] All existing tests pass (they use empty default_args, so fallback behavior is tested)

---

### Task 2: Update API — accept gpu_variant on default-args endpoint

**Context:**
The API endpoint `POST /tama/v1/backends/:name/default-args` currently saves args to the shared `default_args` field. It needs to accept a `gpu_variant` query param and save to `variant_default_args` when provided, falling back to `default_args` when not.

**Files:**
- Modify: `crates/tama-web/src/api/backends/manage.rs`
- Modify: `crates/tama-web/src/api/backends/list.rs`

**What to implement:**
1. In `update_backend_default_args` (manage.rs), add `gpu_variant` query param:
   ```rust
   #[derive(Deserialize)]
   pub struct UpdateDefaultArgsQuery {
       #[serde(default)]
       pub gpu_variant: Option<String>,
   }
   ```
   Update the handler signature to accept the query param. When `gpu_variant` is provided, save to `backend.variant_default_args.insert(gpu_variant, args)`. When not provided, save to `backend.default_args` (backward compat).

2. In `list_backends` (list.rs), when building the `default_args_map`, also build a `variant_args_map` keyed by `"backend_type:gpu_variant"`:
   ```rust
   // Build per-variant args map: "llama_cpp:vulkan" -> Vec<String>
   let variant_args_map: std::collections::HashMap<String, Vec<String>> = config_result
       .ok()
       .iter()
       .flat_map(|cfg| {
           cfg.backends.iter().flat_map(|(bt, bc)| {
               bc.variant_default_args.iter().map(move |(gv, args)| {
                   (format!("{}:{}", bt, gv), args.clone())
               })
           })
       })
       .collect();
   ```
   When building each card, check variant_args_map first, fall back to default_args_map.

**Steps:**
- [ ] Add `UpdateDefaultArgsQuery` struct with `gpu_variant: Option<String>` in `crates/tama-web/src/api/backends/manage.rs`
- [ ] Update `update_backend_default_args` handler to accept `axum::extract::Query(query): axum::extract::Query<UpdateDefaultArgsQuery>`
- [ ] Update the save logic: if `query.gpu_variant` is Some, save to `backend.variant_default_args.insert(gpu_variant, args)`; otherwise save to `backend.default_args`
- [ ] **CRITICAL FIX**: In the same file (manage.rs), find the explicit `BackendConfig` struct construction (~line 810) and add `variant_default_args: BTreeMap::new(),` — otherwise compilation fails because BackendConfig doesn't derive Default
- [ ] In `list_backends` (list.rs), build `variant_args_map` from config
- [ ] When building each card's `default_args`, check `variant_args_map.get(&format!("{}:{}", type_, variant))` first, fall back to `default_args_map.get(&type_)`
- [ ] **ALSO UPDATE**: `check_backend_updates` function (list.rs, ~line 332) — it independently builds its own `default_args_map` and constructs `BackendCardDto` objects. Apply the same `variant_args_map` construction and fallback logic here
- [ ] Run `cargo test --package tama-web --features ssr`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web --features ssr -- -D warnings`
- [ ] Commit with message: "feat: per-variant default_args in API"

**Acceptance criteria:**
- [ ] POST `/tama/v1/backends/:name/default-args?gpu_variant=vulkan` saves to `variant_default_args["vulkan"]`
- [ ] POST `/tama/v1/backends/:name/default-args` (no variant) saves to shared `default_args` (backward compat)
- [ ] GET `/tama/v1/backends` returns per-variant default_args when available, falls back to shared args
- [ ] All existing tests pass

---

### Task 3: Update frontend to send gpu_variant with default-args save

**Context:**
The frontend already keys edits by `"backend_type:gpu_variant"`. The save function extracts the backend type and calls the API. It needs to also pass the `gpu_variant` as a query param so the backend saves to the right variant slot.

**Files:**
- Modify: `crates/tama-web/src/pages/backends.rs`

**What to implement:**
In the save function, when applying default args changes, parse the key `"type:variant"` and pass `gpu_variant` as a query param:
```rust
for (key, args_str) in &*args_edits {
    let parts: Vec<&str> = key.splitn(2, ':').collect();
    let bt = parts[0];
    let url = if let Some(gv) = parts.get(1) {
        format!("/tama/v1/backends/{}/default-args?gpu_variant={}", bt, gv)
    } else {
        format!("/tama/v1/backends/{}/default-args", bt)
    };
    // ... send request
}
```

**Steps:**
- [ ] In the save function's default args loop, parse `key` into `backend_type` and `gpu_variant` using `splitn(2, ':')`
- [ ] Only append `?gpu_variant={}` to the URL when a variant is present (parts.get(1).is_some())
- [ ] Run `cargo check --package tama-web`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Commit with message: "feat: send gpu_variant with default-args save"

**Acceptance criteria:**
- [ ] Saving default args for "llama_cpp:vulkan" hits `/tama/v1/backends/llama_cpp/default-args?gpu_variant=vulkan`
- [ ] Saving default args for "llama_cpp:rocm" hits `/tama/v1/backends/llama_cpp/default-args?gpu_variant=rocm`
- [ ] Each variant's args are saved independently

---

### Task 4: TOML config format & migration

**Context:**
Existing configs have `default_args` as a flat list under each backend. After this change, the config should support per-variant args in TOML format. Existing configs should continue to work (backward compat via `default_args` fallback).

**Files:**
- Modify: `crates/tama-core/src/config/types.rs` (if needed for TOML serialization)

**What to implement:**
The TOML format for per-variant args should look like:
```toml
[backends.llama_cpp]
default_args = ["--threads", "4"]  # fallback for all variants

[backends.llama_cpp.variant_default_args]
vulkan = ["--threads", "8", "--flash-attn"]
rocm = ["--threads", "6"]
cpu = ["--threads", "2"]
```

This is automatically handled by serde's `BTreeMap` serialization — no code changes needed beyond Task 1. Just verify the TOML round-trips correctly.

**Steps:**
- [ ] Write a test that serializes a `BackendConfig` with `variant_default_args` to TOML
- [ ] Verify the TOML format matches the expected structure above
- [ ] Write a test that deserializes the TOML back and values match
- [ ] Run `cargo test --package tama-core`
- [ ] Commit with message: "test: verify variant_default_args TOML round-trip"

**Acceptance criteria:**
- [ ] `BackendConfig` with `variant_default_args` serializes to valid TOML
- [ ] Deserializing the TOML produces the same `variant_default_args` values
- [ ] Existing configs without `variant_default_args` still deserialize correctly (field defaults to empty)

---

### Task 5: Integration test & cleanup

**Context:**
Verify the full flow: edit args per variant in UI → save → reload → each variant shows its own args. Also clean up any unused imports or dead code from the refactoring.

**Files:**
- Modify: `crates/tama-web/tests/server_test.rs` (add integration test)

**What to implement:**
1. Add an integration test that:
   - Creates a temp directory with `config.toml` containing a backend entry
   - Builds a test router with `AppState` pointing at the temp config
   - POSTs default_args with `?gpu_variant=vulkan` → verifies variant_default_args is set
   - GETs `/tama/v1/backends` → verifies per-variant args are returned
   - POSTs default_args without variant → verifies shared default_args is set
   - Verifies fallback: variant without specific args falls back to shared default_args

2. Test setup: The `update_backend_default_args` handler returns 404 when `config_path` is None. Create a temp directory, write a minimal `config.toml` with a `[backends.llama_cpp]` section, and set `config_path` to point at it.

**Steps:**
- [ ] Add integration test in `crates/tama-web/tests/server_test.rs` for per-variant default args
- [ ] Set up temp config directory: `let tmp = tempfile::tempdir().unwrap();` write `config.toml` with backend entry, set `config_path = Some(tmp.path().join("config.toml"))`
- [ ] Run `cargo test --package tama-web --features ssr`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "test: integration test for per-variant default_args"

**Acceptance criteria:**
- [ ] Integration test passes: per-variant args are independent, shared args serve as fallback
- [ ] No clippy warnings
- [ ] All workspace tests pass
