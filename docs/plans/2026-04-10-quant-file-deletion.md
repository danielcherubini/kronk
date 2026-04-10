# Quant File Deletion & Cleanup Plan

**Goal:** Delete GGUF files from disk when users remove quants from the model editor or delete entire models, and add a CLI `prune` command for cleaning up orphaned files.
**Architecture:** Three deletion paths — (1) a new `DELETE /api/models/:id/quants/:quant_key` endpoint for individual quant removal, (2) extend `DELETE /api/models/:id` to also clean up files/DB, and (3) a new `koji model prune` CLI command for bulk orphan cleanup. All share common logic: resolve file path from config, delete file, clean up DB record.
**Tech Stack:** Rust, Axum (web API), Leptos (frontend), rusqlite (DB), inquire (CLI prompts)

---

## Task 1: Add `delete_model_file` DB query to koji-core

**Context:**
The existing `delete_model_records` function deletes ALL DB records for a repo (both `model_pulls` and `model_files`). We need a finer-grained function that deletes a single file's DB record — for when just one quant is removed, not the whole model. This function will be used by the new quant deletion endpoint and by the prune command.

**Files:**
- Modify: `crates/koji-core/src/db/queries/model_queries.rs`
- Test: `crates/koji-core/src/db/queries/model_queries.rs` (inline `#[cfg(test)]`)

**What to implement:**
Add a new public function:

```rust
/// Delete a single model file record by (repo_id, filename).
/// Does NOT touch model_pulls — the repo-level pull record stays.
pub fn delete_model_file(conn: &Connection, repo_id: &str, filename: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM model_files WHERE repo_id = ?1 AND filename = ?2",
        [repo_id, filename],
    )?;
    Ok(())
}
```

This only deletes from `model_files`, not `model_pulls` — the repo-level pull record should remain because other quants for the same repo may still exist.

Add a unit test that:
1. Opens an in-memory SQLite DB, creates the schema via `koji_core::db::migrate()`
2. Inserts a model file record via `upsert_model_file`
3. Calls `delete_model_file` 
4. Verifies the record is gone with `get_model_files`

**Steps:**
- [ ] Write failing test for `delete_model_file` in `crates/koji-core/src/db/queries/model_queries.rs`
- [ ] Run `cargo test --package koji-core -- db::queries::model_queries` and verify test fails
- [ ] Implement `delete_model_file` function
- [ ] Run tests again — verify all pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: `feat: add delete_model_file DB query for single-file cleanup`

**Acceptance criteria:**
- [ ] `delete_model_file` function exists and compiles
- [ ] Unit test passes, verifying a single file record can be deleted without affecting other records

---

## Task 2: Add `DELETE /api/models/:id/quants/:quant_key` endpoint

**Context:**
When a user clicks the ✕ button on a quant row in the model editor, the frontend currently just removes the entry from the local form state and saves the config — the GGUF file stays on disk forever. This task adds a backend endpoint that actually deletes the file and cleans up.

**Files:**
- Modify: `crates/koji-web/src/api.rs` — add the `delete_quant` handler
- Modify: `crates/koji-web/src/server.rs` — add the new route

**What to implement:**

**In `api.rs`**, add a handler function that runs all I/O inside `tokio::task::spawn_blocking` (same pattern as `delete_model`, `update_model`, etc. — never do blocking filesystem/DB ops on the async runtime):

```rust
/// DELETE /api/models/:id/quants/:quant_key — delete a single quant's file
/// and remove it from the config.
pub async fn delete_quant(
    State(state): State<Arc<AppState>>,
    Path((id, quant_key)): Path<(String, String)>,
) -> impl IntoResponse {
```

The handler should:
1. Use `tokio::task::spawn_blocking` for all I/O (file ops, DB ops, config save)
2. Inside the blocking closure, load config via `load_config_from_state(&state)` — clone `state` before entering the closure
3. Find the model by `id` — return 404 `{"error": "Model not found"}` if missing
4. Find the quant entry by `quant_key` in `model.quants` — return 404 `{"error": "Quant not found"}` if missing
5. Clone/save `quant_entry.file` before mutating so we still have the filename after removal
6. Resolve file path: `cfg.models_dir()?.join(&repo_id).join(&quant_entry.file)` where `repo_id` = `model.model` (the HF repo ID, e.g., `"bartowski/OmniCoder-8B-GGUF"`)
7. Delete the file from disk: `if let Err(e) = std::fs::remove_file(&file_path) { tracing::warn!(...) }` — best-effort, don't fail the request
8. Clean up DB: open DB via `koji_core::db::open(&config_dir)`, call `delete_model_file(&conn, &repo_id, &quant_entry.file)` — best-effort, log on error. Note: `config_dir` is the directory containing the config file (from `load_config_from_state`), which is also where `koji.db` lives.
9. If `model.quant == Some(quant_key)`, set `model.quant = None`
10. If `model.mmproj == Some(quant_key)`, set `model.mmproj = None`
11. Remove the quant entry: `model.quants.remove(&quant_key)`
12. **Skip model card update** — the card is managed by CLI `model pull/scan/rm` commands. Editing the card from the web API would add coupling between the web crate and card types. The card will be updated next time `model scan` or `model pull` runs. If the entire model is later deleted (Task 4), the card is deleted too.
13. Save config via `cfg.save_to(&config_dir)`
14. Return `{"ok": true, "id": id, "quant_key": quant_key, "deleted_file": saved_filename}`

**In `server.rs`**, add the route:
```rust
.route("/api/models/:id/quants/:quant_key", delete(api::delete_quant))
```
Place it near the other model routes (after the `/api/models/:id` route block).

**Steps:**
- [ ] Add the route in `crates/koji-web/src/server.rs`
- [ ] Implement `delete_quant` in `crates/koji-web/src/api.rs`
- [ ] Run `cargo build --workspace` — verify compilation
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: `feat: add DELETE /api/models/:id/quants/:quant_key endpoint`

**Acceptance criteria:**
- [ ] `DELETE /api/models/my-server/quants/Q4_K_M` deletes the GGUF file from disk
- [ ] `DELETE /api/models/my-server/quants/Q4_K_M` removes the quant entry from the config
- [ ] `DELETE /api/models/my-server/quants/Q4_K_M` removes the DB `model_files` record
- [ ] Returns 404 if model or quant not found
- [ ] Clears `model.quant` if the deleted quant was the active one
- [ ] Clears `model.mmproj` if the deleted quant was the active mmproj
- [ ] Succeeds even if the file doesn't exist on disk (best-effort deletion)
- [ ] All I/O runs inside `spawn_blocking` — no blocking on the async runtime

---

## Task 3: Frontend — Confirmation dialog and API call on quant delete

**Context:**
Replace the current bare `quants.update(|rows| rows.remove(pos))` handler on the ✕ button with a confirmation dialog followed by an API call to `DELETE /api/models/:id/quants/:quant_key`. This ensures files are actually deleted from disk.

**Files:**
- Modify: `crates/koji-web/src/pages/model_editor.rs`

**What to implement:**

1. **Add a new API function** (near the other API functions like `delete_model_api`, around line 294):

```rust
async fn delete_quant_api(id: String, quant_key: String) -> Result<(), String> {
    let encoded_id = urlencoding::encode(&id);
    let encoded_key = urlencoding::encode(&quant_key);
    let resp = gloo_net::http::Request::delete(
        &format!("/api/models/{}/quants/{}", encoded_id, encoded_key)
    )
    .send()
    .await
    .map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
        Err(text)
    }
}
```

2. **Add a dedicated `Action` for quant deletion** (near the other actions, around line 791):

```rust
let delete_quant_action: Action<(String, String), (), LocalStorage> =
    Action::new_unsync(move |(id, key): &(String, String)| {
        let id = id.clone();
        let key = key.clone();
        async move {
            match delete_quant_api(id, key).await {
                Ok(()) => {
                    // Remove from local state on success
                    quants.update(|rows| {
                        if let Some(pos) = rows.iter().position(|(n, _)| n == &key) {
                            rows.remove(pos);
                        }
                    });
                    // Clear active quant/mmproj references if needed
                    if form_quant.get().as_deref() == Some(key.as_str()) {
                        form_quant.set(None);
                    }
                    if selected_mmproj_for_config.get() == key {
                        selected_mmproj_for_config.set(String::new());
                        form_vision_enabled.set(false);
                    }
                    model_status.set(Some((true, "Quant deleted from disk.".into())));
                }
                Err(e) => {
                    model_status.set(Some((false, format!("Delete failed: {}", e))));
                }
            }
        }
    });
```

3. **Replace the ✕ button's `on:click` handler** (currently at lines 1521-1529). The current code is:

```rust
on:click={
    let name_ref = Arc::clone(&name_arc);
    move |_| {
        quants.update(|rows| {
            if let Some(pos) = rows.iter().position(|(n, _)| n.as_str() == name_ref.as_str()) {
                rows.remove(pos);
            }
        });
    }
}
```

Replace with:

```rust
on:click={
    let name_ref = Arc::clone(&name_arc);
    let size_display = format_bytes_opt(q.size_bytes);
    let persisted_id = original_id.get();
    let key_for_action = name_arc.to_string();
    move |_| {
        let msg = format!(
            "Delete \"{}\" ({}) from disk?\nThis cannot be undone.",
            name_ref.as_str(),
            size_display
        );
        let confirmed = web_sys::window()
            .and_then(|w| w.confirm_with_message(&msg).ok())
            .unwrap_or(false);
        if confirmed {
            delete_quant_action.dispatch((persisted_id, key_for_action));
        }
    }
}
```

Note: We need to compute `size_display` outside of the closure since `q` is a signal that may become stale. Use `let size_display = format_bytes_opt(q.size_bytes);` before the `on:click` block. Actually, since `q` is a `QuantInfo` from the `<For>` iteration, `q.size_bytes` is already accessible. Compute the size string in the view where `q` is in scope, before the button's on:click.

**Important:** The `format_bytes_opt` function and `web_sys::window().confirm_with_message` are already used in this file (see the model delete button at line 1614-1617), so no new imports are needed beyond what's already there. The `urlencoding` crate is also already a dependency (used in refresh/verify).

**Steps:**
- [ ] Add `delete_quant_api` async function near `delete_model_api`
- [ ] Add `delete_quant_action` action near `delete_action`
- [ ] Replace the ✕ button click handler with confirmation + action dispatch
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: `feat: add confirmation dialog + API call for quant file deletion`

**Acceptance criteria:**
- [ ] Clicking ✕ on a quant row shows a browser confirm dialog with the quant name and size
- [ ] Clicking "Cancel" in the dialog does nothing (quant stays)
- [ ] Clicking "OK" calls `DELETE /api/models/:id/quants/:quant_key`
- [ ] On success, the quant row is removed from the shown list
- [ ] If the deleted quant was the active `quant` dropdown selection, it's cleared
- [ ] If the deleted quant was the active `mmproj`, vision is disabled
- [ ] A success/error status message appears after the action

---

## Task 4: Extend `DELETE /api/models/:id` to clean up files and DB

**Context:**
The current `delete_model` handler only removes the config entry. GGUF files, model cards, and DB records are orphaned. This task extends it to clean up everything, mirroring what the CLI `model rm` command does.

**Files:**
- Modify: `crates/koji-web/src/api.rs` — modify `delete_model` handler

**What to implement:**

Extend the `delete_model` handler (currently at line 725). The current code does `if cfg.models.remove(&id).is_none()` which discards the removed model. Restructure it to capture the removed `ModelConfig` so we can use its `model` field for file cleanup.

The existing code is inside `spawn_blocking`, so all file/DB I/O is already on a blocking thread — good, no changes needed there.

Here's the corrected cleanup code to insert after capturing `model_config`:

```rust
let model_config = match cfg.models.remove(&id) {
    Some(m) => m,
    None => return Err(...),
};

// File cleanup (mirrors CLI model rm logic)
let repo_id = model_config.model.as_deref().unwrap_or("");
if !repo_id.is_empty() {
    // 1. Delete model directory: models_dir / repo_id
    if let Ok(models_dir) = cfg.models_dir() {
        let model_dir = models_dir.join(repo_id);
        if model_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&model_dir) {
                tracing::warn!("Failed to remove model directory {}: {}", model_dir.display(), e);
            } else {
                // Clean up empty parent dir
                if let Some(parent) = model_dir.parent() {
                    if parent.read_dir().map(|mut d| d.next().is_none()).unwrap_or(false) {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
            }
        }
    }
    // 2. Delete model card
    if let Ok(configs_dir) = cfg.configs_dir() {
        let card_path = configs_dir.join(format!("{}.toml", repo_id.replace('/', "--")));
        if card_path.exists() {
            let _ = std::fs::remove_file(&card_path);
        }
    }
    // 3. Delete DB records (best-effort)
    // IMPORTANT: `config_dir` is the directory containing config.toml (and koji.db),
    // passed directly — do NOT call .parent() on it. See existing DB calls in this
    // file (lines 321, 861, 937) which all use `config_dir` directly.
    if let Ok(open) = koji_core::db::open(&config_dir) {
        let _ = koji_core::db::queries::delete_model_records(&open.conn, repo_id);
    }
}
```

**Important:** All file/DB cleanup happens BEFORE `cfg.save_to()` but the model was already removed from `cfg.models` in memory. This is fine — even if cleanup fails, we still save the config (best-effort).

**Steps:**
- [ ] Modify `delete_model` handler in `crates/koji-web/src/api.rs`
- [ ] Add `use tracing;` if not already imported
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: `feat: extend model deletion to clean up files, cards, and DB records`

**Acceptance criteria:**
- [ ] `DELETE /api/models/:id` removes the model's GGUF directory from disk
- [ ] Cleans up empty parent directories after deletion
- [ ] Deletes the model card `.toml` file from `configs/`
- [ ] Deletes `model_pulls` and `model_files` DB records for the repo
- [ ] Returns success even if file deletion fails (best-effort cleanup, warnings logged)
- [ ] Returns 404 if model doesn't exist

---

## Task 5: Add `koji model prune` CLI command

**Context:**
There's no way to find and remove orphaned GGUF files — files on disk that aren't referenced by any server config's `quants` map. The existing `koji model scan` does the opposite: it finds untracked files and adds them to model cards. This task adds `koji model prune` which finds orphaned files and removes them.

**Files:**
- Modify: `crates/koji-cli/src/cli.rs` — add `Prune` variant to `ModelCommands`
- Modify: `crates/koji-cli/src/commands/model.rs` — add `cmd_prune` handler

**What to implement:**

**In `cli.rs`**, add to `ModelCommands` enum after the `Scan` variant:

```rust
/// Remove orphaned GGUF files not referenced by any server config
Prune {
    /// Show what would be deleted without actually deleting
    #[arg(long, short = 'n')]
    dry_run: bool,
    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    yes: bool,
},
```

**In `commands/model.rs`**, add a `cmd_prune` function:

1. Build a set of "referenced files" from all server configs:
   - For each `(name, model_config)` in `config.models`:
     - If `model_config.model` is `Some(repo_id)` and `model_config.quants` has entries:
       - For each `(_, quant_entry)` in `model_config.quants`:
         - Add `(repo_id, quant_entry.file)` to the referenced set

2. Scan `models_dir` for orphaned GGUF files:
   - Use the set-difference approach (not directory walking):
   - For each `(repo_id, filename)` in the referenced set, check if `models_dir.join(repo_id).join(filename)` exists. This resolves correctly for both `org/model-GGUF` (3 levels) and `model-GGUF` (2 levels) since `PathBuf::join` handles the `/` separator.
   - Then walk `models_dir` recursively to find ALL `.gguf` files actually on disk. A file is orphaned if its `(repo_id, filename)` pair is NOT in the referenced set. The `repo_id` for a file at `models_dir/org/model/file.gguf` is `"org/model"`, and for `models_dir/model/file.gguf` is `"model"`.
   - **IMPORTANT**: Also scan the `configs/` directory for model cards whose `model.source` corresponds to a model directory that no longer exists or is empty. Report these as orphaned cards.

3. Display orphaned files with sizes (human-readable, using existing formatting helpers)

4. If `dry_run`, print what would be deleted and stop

5. If not `yes`, prompt with `inquire::Confirm`

6. Delete confirmed files:
   - `std::fs::remove_file` for each orphaned GGUF file
   - Clean up empty parent directories (check before removing, like CLI `cmd_rm` does)
   - Clean up DB `model_files` records via `delete_model_file`
   - Remove orphaned model card files

7. Print summary (total files deleted, total space freed)

**In `commands/model.rs`**, add to the dispatch match:
```rust
ModelCommands::Prune { dry_run, yes } => cmd_prune(config, dry_run, yes),
```

**Steps:**
- [ ] Add `Prune` variant to `ModelCommands` in `cli.rs`
- [ ] Add `cmd_prune` function in `commands/model.rs`
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: `feat: add koji model prune command to remove orphaned GGUF files`

**Acceptance criteria:**
- [ ] `koji model prune --dry-run` lists orphaned files without deleting them
- [ ] `koji model prune --yes` deletes all orphaned files without prompting
- [ ] `koji model prune` (no flags) prompts for confirmation before deleting
- [ ] Files referenced by any server config's `quants` map are NOT deleted
- [ ] Empty parent directories are cleaned up after file deletion
- [ ] DB `model_files` records for deleted files are also removed

---

## Task 6: Frontend — Fix model delete to close confirmation gap

**Context:**
Currently the model delete button uses `window.confirm()` and then dispatches `delete_action`, which calls `DELETE /api/models/:id`. After Task 4, that endpoint will now also clean up files. No change needed to the backend call itself, but we should verify the delete confirmation message is clear about file deletion.

**Files:**
- Modify: `crates/koji-web/src/pages/model_editor.rs`

**What to implement:**

Update the model delete confirmation message (line 1615) from:
```rust
"Delete this model? This cannot be undone."
```
to:
```rust
"Delete this model and all its files from disk? This cannot be undone."
```

This is a small change but important for user expectations.

**Steps:**
- [ ] Update the confirmation message in `model_editor.rs`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: `fix: clarify model delete confirmation includes file deletion`

**Acceptance criteria:**
- [ ] The model delete confirmation dialog says "and all its files from disk"