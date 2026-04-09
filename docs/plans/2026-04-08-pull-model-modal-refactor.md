# Pull Model Modal Refactor Plan

**Goal:** Remove the redundant `/pull` page and replace it with a modal on the Models page that launches the existing `PullQuantWizard`.

**Architecture:** The `PullQuantWizard` component already supports being hosted in a modal — it has `is_open`, `on_close`, and `on_complete` props used today in `model_editor.rs`. We replicate that exact pattern in `models.rs`, passing an empty `initial_repo` so the wizard starts at its built-in Step 1 (RepoInput — a repo text field + Search button). However, the wizard's Reset Effect currently early-returns when `initial_repo` is empty, which would leave stale state on reopen; Task 2 fixes this before wiring up the modal. The `/pull` route, nav link, page file, and `+ New Model` button are all removed.

**Tech Stack:** Rust, Leptos 0.7, WASM frontend (`crates/koji-web`)

**Reference implementation:** `crates/koji-web/src/pages/model_editor.rs` lines 1356–1365 shows the exact pattern: a `Modal` wrapping `PullQuantWizard` with `initial_repo`, `is_open`, `on_complete`, and `on_close` props.

---

### Task 1: Remove the `/pull` page, route, and nav link

**Context:**
The current app has a dedicated top-level "Pull Model" page at `/pull` which wraps `PullQuantWizard`. This is being replaced by a modal on the Models page. This task removes all traces of the old page: the file, its module declaration, the router entry, and the nav bar link. After this task the app will compile and run but without "Pull Model" in the nav or as a route.

**Files:**
- Delete: `crates/koji-web/src/pages/pull.rs`
- Modify: `crates/koji-web/src/pages/mod.rs`
- Modify: `crates/koji-web/src/lib.rs`
- Modify: `crates/koji-web/src/components/nav.rs`

**What to implement:**

1. **Delete `crates/koji-web/src/pages/pull.rs`** entirely. It is a 16-line file that just renders `PullQuantWizard` on a page — no logic to preserve.

2. **`crates/koji-web/src/pages/mod.rs`** — remove `pub mod pull;`. The file currently has 6 lines, one per page module. After the change it should have 5 lines.

3. **`crates/koji-web/src/lib.rs`** — remove the route:
   ```rust
   <Route path=path!("/pull") view=pages::pull::Pull />
   ```
   This is line 31. Do NOT remove any other routes. The `/models/:id/edit` route stays — it is still used for editing existing models via the Edit button on each model card.

4. **`crates/koji-web/src/components/nav.rs`** — remove the nav link:
   ```rust
   <A href="/pull" attr:class="nav-link">"Pull Model"</A>
   ```
   This is line 11. Do NOT remove any other nav links.

**Steps:**
- [ ] Delete `crates/koji-web/src/pages/pull.rs`
- [ ] Remove `pub mod pull;` from `crates/koji-web/src/pages/mod.rs`
- [ ] Remove the `/pull` route from `crates/koji-web/src/lib.rs`
- [ ] Remove the "Pull Model" nav link from `crates/koji-web/src/components/nav.rs`
- [ ] Run `cargo build -p koji-web`
  - Did it succeed? If there are compile errors about missing `pull` module or `Pull` component, check the above four files. Fix and re-run before continuing.
- [ ] Run `cargo clippy -p koji-web -- -D warnings`
  - Did it succeed? Fix any warnings before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat: remove /pull page, route, and nav link`

**Acceptance criteria:**
- [ ] `crates/koji-web/src/pages/pull.rs` no longer exists
- [ ] `crates/koji-web/src/pages/mod.rs` has no `pull` module
- [ ] `crates/koji-web/src/lib.rs` has no `/pull` route
- [ ] `crates/koji-web/src/components/nav.rs` has no "Pull Model" link
- [ ] `cargo build -p koji-web` succeeds with no errors
- [ ] `cargo clippy -p koji-web -- -D warnings` passes

---

### Task 2: Fix `PullQuantWizard` reset behaviour for empty `initial_repo`

**Context:**
`PullQuantWizard`'s Reset Effect (the `Effect::new` block registered when `is_open` is `Some`) currently early-returns when `initial_repo` is empty, leaving all wizard signals (step, repo_id, quants, download_jobs, etc.) untouched. When hosted in a modal with an empty `initial_repo` (as Task 3 will do), this causes two broken flows:

- **After completion:** User opens modal → completes a pull → modal auto-closes. User opens modal again → wizard is still showing the Done screen from the previous session.
- **After cancel mid-flow:** User opens modal → enters repo → reaches SelectQuants → clicks Cancel → modal closes. User reopens → wizard is still on SelectQuants showing stale data.

The fix: when `is_open` transitions to `true` and the wizard is at `RepoInput` or `Done` (i.e., not mid-flow), always reset all state signals back to their defaults and set the step to `RepoInput` — even when `initial_repo` is empty. Only skip the auto-fetch when the repo is empty. This preserves the existing `model_editor.rs` behaviour (non-empty repo → reset + auto-fetch) and adds the new behaviour (empty repo → reset only, start at RepoInput).

This change must NOT break `model_editor.rs`'s use of the wizard, which passes a non-empty `initial_repo`.

**Files:**
- Modify: `crates/koji-web/src/components/pull_quant_wizard.rs`

**What to implement:**

Find the Reset Effect block starting at line 224. Currently it looks like:

```rust
if let Some(is_open_sig) = is_open {
    Effect::new(move |_| {
        let open = is_open_sig.get();
        if !open {
            return;
        }
        let repo = initial_repo.get_untracked();
        if repo.trim().is_empty() {
            return;    // ← THIS early return is the bug
        }
        let step = wizard_step.get_untracked();
        if !matches!(step, WizardStep::RepoInput | WizardStep::Done) {
            // Mid-flow session — preserve it across close/reopen.
            return;
        }
        // Reset session state.
        selected_filenames.set(std::collections::HashSet::new());
        selected_mmproj_filenames.set(std::collections::HashSet::new());
        context_lengths.set(std::collections::HashMap::new());
        download_jobs.set(Vec::new());
        error_msg.set(None);
        did_complete.set(false);
        repo_id.set(repo.clone());
        wizard_step.set(WizardStep::LoadingQuants);

        // Spawn the same fetch the Search button does today.
        wasm_bindgen_futures::spawn_local(async move { ... });
    });
}
```

Replace with this logic (the reset now happens unconditionally when the wizard is at RepoInput/Done; only the auto-fetch is skipped for empty repos):

```rust
if let Some(is_open_sig) = is_open {
    Effect::new(move |_| {
        let open = is_open_sig.get();
        if !open {
            return;
        }
        let step = wizard_step.get_untracked();
        if !matches!(step, WizardStep::RepoInput | WizardStep::Done) {
            // Mid-flow session — preserve it across close/reopen.
            return;
        }
        // Always reset session state when (re)opening at a terminal step.
        selected_filenames.set(std::collections::HashSet::new());
        selected_mmproj_filenames.set(std::collections::HashSet::new());
        context_lengths.set(std::collections::HashMap::new());
        download_jobs.set(Vec::new());
        error_msg.set(None);
        did_complete.set(false);
        wizard_step.set(WizardStep::RepoInput);

        let repo = initial_repo.get_untracked();
        if repo.trim().is_empty() {
            return;  // No auto-fetch for empty repo — user will type one in.
        }
        repo_id.set(repo.clone());
        wizard_step.set(WizardStep::LoadingQuants);

        // Spawn the same fetch the Search button does today.
        wasm_bindgen_futures::spawn_local(async move { ... });
    });
}
```

Do NOT change the spawn_local block contents — only reorder the guard and reset logic as shown above. The `...` placeholder represents the existing unchanged async fetch code.

**Steps:**
- [ ] Locate the Reset Effect block in `pull_quant_wizard.rs` (starts around line 224 with `if let Some(is_open_sig) = is_open {`)
- [ ] Reorder the logic as described: move the step guard before the repo check; always reset state signals; set step to `RepoInput` unconditionally; only skip the auto-fetch when repo is empty; set `repo_id` and `LoadingQuants` before spawning the fetch
- [ ] Run `cargo build -p koji-web`
  - Did it succeed? Fix any compile errors and re-run before continuing.
- [ ] Run `cargo clippy -p koji-web -- -D warnings`
  - Did it succeed? Fix any warnings before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `fix: reset PullQuantWizard state on reopen when initial_repo is empty`

**Acceptance criteria:**
- [ ] The Reset Effect resets all session signals (step, filenames, context_lengths, download_jobs, error_msg, did_complete) whenever `is_open` transitions to `true` and the wizard is at `RepoInput` or `Done`
- [ ] When `initial_repo` is empty after the reset, the wizard ends at `WizardStep::RepoInput` and no fetch is spawned
- [ ] When `initial_repo` is non-empty (the `model_editor.rs` case), the wizard still resets and then auto-fetches exactly as before
- [ ] `cargo build -p koji-web` and `cargo clippy -p koji-web -- -D warnings` both succeed

---

### Task 3: Add "Pull Model" modal to the Models page

**Context:**
The Models page (`crates/koji-web/src/pages/models.rs`) currently has a `+ New Model` button that links to `/models/new/edit` (a route being retired). We replace it with a "Pull Model" button that opens a modal containing `PullQuantWizard` (now fixed in Task 2 to reset correctly on reopen). We also update the empty-state fallback (which previously linked to `/pull`) to use the same modal.

The `Modal` component (`crates/koji-web/src/components/modal.rs`) takes `open: Signal<bool>`, `on_close: Callback<()>`, `title: String`, and `children: ChildrenFn`.

The existing pattern in `model_editor.rs` uses a local helper `rw_signal_to_signal` to convert an `RwSignal<T>` to `Signal<T>`:
```rust
fn rw_signal_to_signal<T: Clone + Send + Sync + 'static>(sig: RwSignal<T>) -> Signal<T> {
    let (read, _) = sig.split();
    read.into()
}
```
Add this same helper (module-private `fn`) near the top of `models.rs`.

**Important:** The `use leptos_router::components::A;` import in `models.rs` must be **kept**. The Edit buttons on each model card (both in the "Loaded Models" section at line 153 and the "Unloaded Models" section at line 233) still use `<A href=format!("/models/{}/edit", id_edit)>`. Only the `<A href="/models/new/edit">` wrapper around the `+ New Model` button is being removed.

**Files:**
- Modify: `crates/koji-web/src/pages/models.rs`

**What to implement:**

**Imports to add at the top of `models.rs` (keep all existing imports):**
```rust
use crate::components::modal::Modal;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
```
Keep the existing `use leptos_router::components::A;` — the Edit buttons still use `<A>`.

**Local helper — add before the `Models` component fn:**
```rust
fn rw_signal_to_signal<T: Clone + Send + Sync + 'static>(sig: RwSignal<T>) -> Signal<T> {
    let (read, _) = sig.split();
    read.into()
}
```

**State — add inside `Models` component, alongside the existing `refresh` signal:**
```rust
let pull_modal_open = RwSignal::new(false);
```

**Page header — replace the `+ New Model` button block:**

Current (lines 62–67):
```rust
<div class="page-header">
    <h1>"Models"</h1>
    <A href="/models/new/edit">
        <button class="btn btn-primary">"+ New Model"</button>
    </A>
</div>
```

Replace with:
```rust
<div class="page-header">
    <h1>"Models"</h1>
    <button class="btn btn-primary" on:click=move |_| pull_modal_open.set(true)>
        "Pull Model"
    </button>
</div>
```

**Empty-state fallback — replace the `/pull` anchor (line 80):**

Current:
```rust
<a href="/pull"><button class="btn btn-primary mt-2">"Pull a Model"</button></a>
```

Replace with:
```rust
<button class="btn btn-primary mt-2" on:click=move |_| pull_modal_open.set(true)>
    "Pull a Model"
</button>
```

**Modal mount — add after the closing `</Suspense>` tag:**
```rust
<Modal
    open=rw_signal_to_signal(pull_modal_open)
    on_close=Callback::new(move |_| pull_modal_open.set(false))
    title="Pull Model".to_string()
>
    <PullQuantWizard
        initial_repo=Signal::derive(String::new)
        is_open=rw_signal_to_signal(pull_modal_open)
        on_complete=Callback::new(move |_completed: Vec<CompletedQuant>| {
            pull_modal_open.set(false);
            refresh.update(|n| *n += 1);
        })
        on_close=Callback::new(move |_| pull_modal_open.set(false))
    />
</Modal>
```

The `on_complete` callback auto-closes the modal and increments `refresh` so the models list refetches. The `on_close` callback (fired by the wizard's Cancel button) just closes the modal without a refresh. `initial_repo=Signal::derive(String::new)` passes an empty string, so the wizard (after the Task 2 fix) starts fresh at the RepoInput step each time the modal opens.

**Steps:**
- [ ] Add the two new `use` imports at the top of `models.rs` (keep `use leptos_router::components::A;`)
- [ ] Add the `rw_signal_to_signal` helper function before the `Models` component
- [ ] Add `let pull_modal_open = RwSignal::new(false);` inside `Models`
- [ ] Replace the `+ New Model` button block (lines 62–67) with the `Pull Model` button
- [ ] Replace the empty-state `<a href="/pull">` anchor (line 80) with the modal-opening button
- [ ] Add the `Modal` + `PullQuantWizard` block after `</Suspense>`
- [ ] Run `cargo build -p koji-web`
  - Did it succeed? Fix compile errors and re-run before continuing.
- [ ] Run `cargo clippy -p koji-web -- -D warnings`
  - Did it succeed? Fix any warnings before continuing.
- [ ] Run `cargo test -p koji-web`
  - The existing `partition_models_by_loaded` unit tests in `models.rs` must still pass. Fix regressions before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat: add Pull Model modal to Models page`

**Acceptance criteria:**
- [ ] Models page has a "Pull Model" button in the page header
- [ ] Clicking "Pull Model" opens a modal with the `PullQuantWizard` starting at the repo input step
- [ ] Empty-state "Pull a Model" button also opens the same modal
- [ ] `+ New Model` button and its `<A href="/models/new/edit">` wrapper are fully removed
- [ ] `use leptos_router::components::A;` import is still present (Edit buttons still need it)
- [ ] When the wizard completes downloads, the modal auto-closes and the model list refreshes
- [ ] When the user clicks Cancel in the wizard, the modal closes without refresh
- [ ] All existing `partition_models_by_loaded` unit tests pass
- [ ] `cargo build -p koji-web` and `cargo clippy -p koji-web -- -D warnings` both succeed

---

### Task 4: Update docs/plans/README.md with deferred cleanup note

**Context:**
Removing the `/models/new/edit` route in this refactor leaves dead code in `model_editor.rs`. The `is_new` mode branches (multiple call sites: lines ~92, 187, 206, 280, 629, 673, 701, 1330) are now unreachable since no route or button leads to `/models/new/edit` anymore. Stripping these branches is a non-trivial change to the model save logic and deserves its own focused PR, so it is explicitly deferred here with a tracked note. Also update the plan index stats and add this plan to the Web UI section.

**Files:**
- Modify: `docs/plans/README.md`

**What to implement:**

1. Add a **"Deferred Cleanup"** section just before the "Related Documentation" section:

```markdown
## Deferred Cleanup

These are known cleanup items that have been intentionally deferred for focused follow-up PRs.

| Item | Description |
|------|-------------|
| Strip `is_new` code paths from `model_editor.rs` | When `/models/new/edit` was removed (pull-modal refactor, 2026-04-08), the `is_new` branches in `model_editor.rs` became dead code (call sites at lines ~92, 187, 206, 280, 629, 673, 701, 1330). Remove them: the `is_new` local signal, the `save_model(is_new: bool)` parameter, the POST-vs-PUT branching, and the `"New Model"` heading branch. |
```

2. Update **Quick Stats** section — the current numbers are `Total Plans: 27`, `Completed: 26`, `Remaining: 1`. Change to: `Total Plans: 28`, `Completed: 27`, `Remaining: 1`.

3. Add an entry to the **"Web UI"** table under "Completed Plans" (the table that currently has "Web UI Redesign" and "Config Hot Reload"):

```markdown
| [Pull Model Modal Refactor](2026-04-08-pull-model-modal-refactor.md) | Remove /pull page; relocate PullQuantWizard into modal on Models page; remove + New Model button | _(TBD)_ |
```

4. Bump **Last Updated** at the bottom from `2026-04-06` to `2026-04-08`.

**Steps:**
- [ ] Add the "Deferred Cleanup" section before "Related Documentation"
- [ ] Update Quick Stats: Total Plans 27→28, Completed 26→27
- [ ] Add the pull-model plan entry to the Web UI table
- [ ] Bump Last Updated to 2026-04-08
- [ ] Commit with message: `docs: add pull-model modal refactor plan and deferred cleanup note`

**Acceptance criteria:**
- [ ] `docs/plans/README.md` contains a "Deferred Cleanup" section with the `is_new` cleanup item
- [ ] Quick Stats shows Total Plans: 28, Completed: 27
- [ ] The pull-model plan appears in the Web UI table
- [ ] Last Updated is 2026-04-08
