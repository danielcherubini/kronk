# Fix Backend Default Args & Move Save Button — Plan

**Goal:** Fix the bug where backend default_args are never displayed in the webui (due to passing file path instead of dir path to `Config::load_from`), and restructure the UI to use a single page-level Save button matching the config editor pattern.

**Architecture:** The API bug is a simple argument swap — `config_path` → `config_dir` in 4 locations. The frontend change lifts per-card save logic out of `BackendCard` into the `Backends` page, matching the config editor's single-save-button pattern.

**Tech Stack:** Rust (Axum server), Leptos 0.7 (WASM frontend), serde_json, gloo-net

---

## Task 1: Fix `Config::load_from`/`save_to` path bugs in API

**Context:**
Four call sites in `crates/koji-web/src/api/backends.rs` pass the full file path (`config_path`) to functions that expect a directory path (`config_dir`). This causes `default_args` to never load from disk (silently fails), and saving also fails. The `config_dir` variable is already computed but unused in two of these locations. The other two locations (`update_backend_default_args`) don't compute `config_dir` at all.

**Files:**
- Modify: `crates/koji-web/src/api/backends.rs`

**What to implement:**

1. **Line 444 — `list_backends`**: Change `koji_core::config::Config::load_from(&config_path)` to `koji_core::config::Config::load_from(&config_dir)`. The `config_dir` variable is already computed on line 422-431.

2. **Line 1211 — `check_backend_updates`**: Change `koji_core::config::Config::load_from(&config_path)` to `koji_core::config::Config::load_from(&config_dir)`. The `config_dir` variable is already computed on line 1189-1198.

3. **Lines 1503 & 1534 — `update_backend_default_args`**: This function does not currently compute `config_dir`. Add it:
   - After extracting `config_path` (line 1491-1500), add:
     ```rust
     let config_dir = match config_path.parent() {
         Some(d) => d.to_path_buf(),
         None => {
             return (
                 StatusCode::INTERNAL_SERVER_ERROR,
                 Json(serde_json::json!({"error": "Cannot determine config directory"})),
             )
                 .into_response();
         }
     };
     ```
   - Change line 1503: `Config::load_from(&config_path)` → `Config::load_from(&config_dir)`
   - Change line 1534: `config.save_to(&config_path)` → `config.save_to(&config_dir)`

**Steps:**
- [ ] Make the 4 changes described above in `crates/koji-web/src/api/backends.rs`
- [ ] Run `cargo check --workspace`
  - If it fails, fix errors and re-run before continuing.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `fix: use config_dir instead of config_path in backend API handlers`

**Acceptance criteria:**
- [ ] `Config::load_from` is called with `config_dir` (directory path) in all 4 locations
- [ ] `Config::save_to` is called with `config_dir` in `update_backend_default_args`
- [ ] All workspace tests pass

---

## Task 2: Remove `skip_serializing_if` from `default_args` DTO fields

**Context:**
Both the server-side and client-side `BackendCardDto` use `#[serde(default, skip_serializing_if = "Vec::is_empty")]` on `default_args`. When `default_args` is an empty vec (which is the common case due to the bug), the field is omitted from the JSON response entirely. Even after fixing the bug, backends with genuinely empty `default_args` would have the field missing. Removing `skip_serializing_if` ensures the field is always present, matching how the config editor handles fields.

**Files:**
- Modify: `crates/koji-web/src/api/backends.rs`
- Modify: `crates/koji-web/src/components/backend_card.rs`

**What to implement:**

1. In `crates/koji-web/src/api/backends.rs`, find the struct `BackendCardDto` (around line 44-48). Change:
   ```rust
   #[serde(default, skip_serializing_if = "Vec::is_empty")]
   pub default_args: Vec<String>,
   ```
   to:
   ```rust
   #[serde(default)]
   pub default_args: Vec<String>,
   ```

2. In `crates/koji-web/src/components/backend_card.rs`, find the struct `BackendCardDto` (around line 83-84). Make the same change:
   ```rust
   #[serde(default, skip_serializing_if = "Vec::is_empty")]
   pub default_args: Vec<String>,
   ```
   →
   ```rust
   #[serde(default)]
   pub default_args: Vec<String>,
   ```

**Steps:**
- [ ] Remove `skip_serializing_if = "Vec::is_empty"` from `default_args` in both files
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `fix: always include default_args in backend DTO JSON`

**Acceptance criteria:**
- [ ] `default_args` field is always present in JSON responses, even when empty
- [ ] `#[serde(default)]` is still present (ensures backward compat for deserialization)

---

## Task 3: Restructure BackendCard — remove per-card save, add page-level save

**Context:**
Currently `BackendCard` contains per-card save logic: a "Save" button and an `on:blur` handler that both POST directly to `/api/backends/{name}/default-args`. This needs to be replaced with a pattern where the `Backends` page manages all edits and has a single "Save Changes" button.

The config editor page (`config_editor.rs`) provides the proven pattern:
- Page-level `RwSignal<Option<Config>>` holds edit state
- `on:input`/`on:change` on form fields update the signal directly
- Single "Save Changes" button in `.page-header` serializes the signal and POSTs
- `save_status` signal shows feedback

For the backends page, we need:
- A `default_args_edits: RwSignal<HashMap<String, String>>` in the `Backends` component to track which backend args have been edited (backend_type → args string)
- An `on_default_args_change: Callback<(String, String)>` prop on `BackendCard` that calls back to the page with `(backend_type, new_value)`
- Remove `on:blur` save and "Save" button from `BackendCard`
- Add "Save Changes" button to `Backends` page `.page-header`
- On save, POST each changed backend's args sequentially, show feedback

**Files:**
- Modify: `crates/koji-web/src/components/backend_card.rs`
- Modify: `crates/koji-web/src/pages/backends.rs`

**What to implement:**

#### backend_card.rs changes:

1. **Add `on_default_args_change` callback prop** to the `BackendCard` component:
   ```rust
   #[prop(optional)]
   on_default_args_change: Option<Callback<(String, String)>>,
   ```

2. **Keep `default_args_signal`** for the input's `prop:value` binding so the input displays the current value. Remove all save-related logic from it — it should only update the local signal and call the parent callback:
    ```rust
    let default_args_initial = backend.default_args.join(" ");
    let default_args_signal = RwSignal::new(default_args_initial);
    let bt_input = backend.r#type.clone();
    ```

3. **Replace the `on:blur` handler** (lines 182-195) with an `on:input` handler that updates the local signal and calls the parent callback:
    ```rust
    on:input=move |ev| {
        if let Some(input) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
            default_args_signal.set(input.value());
            if let Some(cb) = &on_default_args_change {
                cb.run((bt_input.clone(), input.value()));
            }
        }
    }
    ```
    Note: Also remove the `on:blur` handler entirely (lines 182-195).

4. **Remove the "Save" button** (lines 197-215) — the entire `<button>` element from inside the `<div>` that wraps the default_args input section.

#### backends.rs changes:

1. **Add edit tracking signals:**
   ```rust
   let default_args_edits: RwSignal<std::collections::HashMap<String, String>> = RwSignal::new(std::collections::HashMap::new());
   let save_status: RwSignal<Option<String>> = RwSignal::new(None);
   let saving: RwSignal<bool> = RwSignal::new(false);
   ```
   - `default_args_edits`: tracks which backend args have been edited (backend_type → args string)
   - `save_status`: displays "Saving…", "✅ Saved", or "❌ error" next to the button
   - `saving`: prevents double-clicks from firing concurrent saves

2. **Add `on_default_args_change` callback:**
   ```rust
   let on_default_args_change = Callback::new(move |(backend_type, new_value): (String, String)| {
       default_args_edits.update(|edits| {
           edits.insert(backend_type, new_value);
       });
       save_status.set(None); // Clear status when user makes new edits
   });
   ```

3. **Add save handler** that posts each edited backend's args, with a `saving` guard:
   ```rust
   let save = move |_| {
       if saving.get() { return; } // Guard against concurrent saves
       let edits = default_args_edits.get();
       if edits.is_empty() {
           return;
       }
       saving.set(true);
       save_status.set(Some("Saving…".to_string()));
       wasm_bindgen_futures::spawn_local(async move {
           let mut errors = Vec::new();
           let edit_keys: Vec<String> = edits.keys().cloned().collect();
           for bt in edit_keys {
               let args_str = edits.get(&bt).cloned().unwrap_or_default();
               let parts: Vec<String> = args_str.split_whitespace().map(String::from).collect();
               let body = serde_json::json!({ "default_args": parts });
               let url = format!("/api/backends/{}/default-args", bt);
               let res = gloo_net::http::Request::post(&url)
                   .json(&body)
                   .unwrap()
                   .send()
                   .await;
               if let Err(e) = res {
                   errors.push(format!("{}: {}", bt, e));
               }
           }
           if errors.is_empty() {
               save_status.set(Some("✅ Saved".to_string()));
               default_args_edits.set(std::collections::HashMap::new());
               refresh_tick.update(|n| *n += 1);
           } else {
               save_status.set(Some(format!("❌ {}", errors.join(", "))));
           }
           saving.set(false);
       });
   };
   ```

4. **Update the `.page-header`** to add a "Save Changes" button next to the `<h1>`. Show the button conditionally when there are pending edits. Disable it while saving:
   ```rust
   <div class="page-header">
       <h1>"Backends"</h1>
       {move || if !default_args_edits.get().is_empty() || saving.get() {
           view! {
               <div style="display:flex;gap:0.5rem;align-items:center;">
                   {move || save_status.get().map(|s| view! { <span class="text-muted">{s}</span> })}
                   <button
                       class="btn btn-primary"
                       disabled=move || saving.get()
                       on:click=save
                   >
                       "Save Changes"
                   </button>
               </div>
           }.into_any()
       } else {
           view! { <span/> }.into_any()
       }}
   </div>
   ```
   Note: The save button is conditionally shown when there are edits OR when a save is in progress (so the "Saving…" status remains visible). This intentionally differs from the config editor which always shows the button — here we only show it when relevant.

5. **Pass `on_default_args_change` to `BackendCard`**:
   ```rust
   <BackendCard
       backend=backend
       on_install=on_install_click
       on_update=on_update_click
       on_check_updates=on_check_updates_click
       on_delete=on_delete_click
       on_default_args_change=on_default_args_change
   />
   ```

6. **Clear `default_args_edits` after successful save** (already shown in step 3) and trigger a refresh to get fresh data from the server.

**Steps:**
- [ ] Modify `backend_card.rs`: Add `on_default_args_change` callback prop, remove `on:blur` save and "Save" button, update `on:input` to call callback
- [ ] Modify `backends.rs`: Add `default_args_edits` and `save_status` signals, add save callback, update page header, pass new prop to BackendCard
- [ ] Run `cargo check --package koji-web`
  - Fix any compile errors before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Fix any warnings.
- [ ] Commit with message: `feat: move backend default_args save to page-level button`

**Acceptance criteria:**
- [ ] No per-card "Save" button present in BackendCard component
- [ ] No `on:blur` save handler on default_args input
- [ ] Single "Save Changes" button in the backends page header
- [ ] Save button appears only when there are unsaved default_args edits
- [ ] Save button shows "Saving…", "✅ Saved", or "❌ error" feedback
- [ ] Successful save refreshes the backends list and clears the edits map

---

## Task 4: End-to-end manual verification

**Context:**
The changes span both server and client side. We need to verify the full flow works: config loads `default_args` correctly, API serves them, frontend displays them, and saving updates them.

**Steps:**
- [ ] Run `cargo build --workspace`
  - If it fails, fix errors and retry.
- [ ] Run `cargo test --workspace`
  - All tests must pass.
- [ ] Start the server locally and verify:
  1. Navigate to the Backends page in the web UI
  2. Confirm `default_args` values from `config.toml` appear in the input fields
  3. Edit a backend's default_args
  4. Click "Save Changes" in the page header
  5. Verify "✅ Saved" appears
  6. Verify the backends list refreshes with the updated values
  7. Verify `config.toml` on disk was updated
- [ ] Commit any fixes with message: `chore: verification fixes for backend default_args`

**Acceptance criteria:**
- [ ] All workspace tests pass
- [ ] Workspace builds successfully
- [ ] No clippy warnings