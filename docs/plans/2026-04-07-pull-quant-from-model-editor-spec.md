# Pull Quant from HuggingFace on the Model Editor — Spec

**Status:** Draft (rev 2 — incorporates reviewer feedback)
**Date:** 2026-04-07
**Branch:** `feature/pull-quant-from-model-editor`

## 1. Goal & UX summary

Replace the bare **+ Add Quant** button on the model editor's Quantizations table
with a **+ Pull Quant** button that opens a modal-hosted wizard. The wizard
reuses the existing HuggingFace pull flow (browse available quants, pick context
length, download with live progress). When done, the newly downloaded quants are
merged into the editor's local quants signal so the user sees them immediately
without losing any unsaved form edits.

The button is **disabled** when the editor's `model` field (HF repo ID) is
empty, with a tooltip nudging the user to fill it in first.

## 2. Background

### Current state on the model edit page (`/models/:id`)

- The "Quantizations" table at the bottom lets you manually edit `name`,
  `file`, `size_bytes`, `context_length` for each quant.
- The current **+ Add Quant** button (in `crates/koji-web/src/pages/model_editor.rs`
  around line 1170) just appends an empty row with a placeholder name like
  `quant-N`. The user has to type in the GGUF filename, look up the size by
  hand, etc.
- The model already knows its HF repo via the `form_model` signal (e.g.
  `bartowski/Qwen3-8B-GGUF`).

### Existing pull-from-HF flow (`/pull` page)

- 6-step wizard in `crates/koji-web/src/pages/pull.rs`:
  `RepoInput → LoadingQuants → SelectQuants → SetContext → Downloading → Done`
- Backed by:
  - `GET /koji/v1/hf/*repo_id` — lists quants on HF
    (`crates/koji-core/src/proxy/koji_handlers.rs::handle_hf_list_quants`).
    Note: this calls `fetch_blob_metadata` directly and does **not** do
    `-GGUF` auto-resolution. See §12 (out-of-scope notes).
  - `POST /koji/v1/pulls` — starts download jobs
  - `GET /koji/v1/pulls/:job_id/stream` — SSE progress stream
- On success, the backend handler `setup_model_after_pull`
  (`crates/koji-core/src/proxy/koji_handlers.rs`) writes/updates the model card
  on disk **and** inserts/updates the entry into the live in-memory config.
  Critically, this is invoked unconditionally on download success — independent
  of any client.

### Key insight

The backend already does the right thing. When a pull completes, the new quants
are merged into the existing model card and live config. **No backend changes
are required.** This is purely a Leptos client-side refactor + UI feature.

## 3. Design decisions

| # | Decision | Choice |
|---|---|---|
| 1 | Where the wizard lives | Extract into a reusable Leptos component, embed in both `/pull` and the model editor |
| 2 | How it appears in the model editor | Modal overlay (dim background) |
| 3 | Whether to filter already-pulled quants | Show all, allow re-pull (matches current `/pull` behavior; backend upserts) |
| 4 | What happens after download completes | Client-side merge of completed quants into the local `quants` signal (preserves unsaved form edits) |
| 5 | What the button does when `form_model` is empty | Disabled, with a tooltip |
| 6 | Modal close-during-download semantics | **Modal renders always**, toggles `display:none` when closed. Wizard is never unmounted; SSE futures keep running across close/reopen. |

## 4. Scope of work

| Area | Change |
|---|---|
| New shared component | `crates/koji-web/src/components/pull_quant_wizard.rs` — extracted from the bulk of `pages/pull.rs` |
| New shared component | `crates/koji-web/src/components/modal.rs` — first modal in the codebase, reusable shell |
| Refactored | `crates/koji-web/src/pages/pull.rs` — becomes a thin wrapper that mounts the wizard with no pre-set repo |
| Modified | `crates/koji-web/src/pages/model_editor.rs` — adds modal trigger, mounts wizard with `form_model` pre-set, handles `on_complete` callback |
| Modified | `crates/koji-web/style.css` — modal overlay styles |
| Module wiring | `crates/koji-web/src/components/mod.rs` — `pub mod modal; pub mod pull_quant_wizard;` |

**Out of scope:** any backend changes. The existing `GET /koji/v1/hf/*repo_id`,
`POST /koji/v1/pulls`, `GET /koji/v1/pulls/:job_id/stream`, and the post-download
`setup_model_after_pull` machinery already do exactly what we need.

## 5. The reusable `PullQuantWizard` component

Location: `crates/koji-web/src/components/pull_quant_wizard.rs`.

The current `pages/pull.rs` already contains the full state machine — we hoist
it almost verbatim and parameterize the entry point.

### 5.1 Props

```rust
#[component]
pub fn PullQuantWizard(
    /// Pre-set HF repo ID. If non-empty, the wizard skips step 1 and immediately
    /// fetches quants. If empty, the wizard starts at the repo-input step.
    #[prop(into)] initial_repo: Signal<String>,

    /// Whether the wizard is currently visible to the user. The wizard uses this
    /// to detect (closed → open) transitions and reset itself when reopened
    /// after a previous Done state. When the wizard is hosted directly on a page
    /// (not in a modal), pass `Signal::derive(|| true)`.
    #[prop(into, optional)] is_open: Option<Signal<bool>>,

    /// Called once after ALL downloads in the current session have reached a
    /// terminal state (Completed or Failed). Receives the list of quants that
    /// completed successfully so the host can merge them into its own state.
    /// Not called if the user dismisses the wizard before any downloads start.
    /// Fires exactly once per session — guarded by an internal `did_complete`
    /// flag that resets on (closed → open) transition.
    #[prop(optional)] on_complete: Option<Callback<Vec<CompletedQuant>>>,

    /// Called when the user dismisses the wizard via an in-step Cancel/Close
    /// button. The host decides whether to close the modal — wizard never
    /// hides itself.
    #[prop(optional)] on_close: Option<Callback<()>>,
) -> impl IntoView
```

### 5.2 Public type emitted on completion

```rust
#[derive(Clone, Debug)]
pub struct CompletedQuant {
    /// Exactly the repo_id the wizard used (no -GGUF auto-resolution happens
    /// today; see §12). Provided so the host has the full context if needed.
    pub repo_id: String,

    /// e.g. "Qwen3-8B-Q4_K_M.gguf"
    pub filename: String,

    /// e.g. "Q4_K_M" — derived using the same logic as the backend's
    /// `infer_quant_from_filename` (see §5.5).
    pub quant: Option<String>,

    /// Final downloaded byte count, sourced from the SSE done payload's
    /// `bytes_downloaded` field. This equals the actual file size on disk
    /// because `download_chunked` writes bytes 1:1 as they arrive.
    /// `None` only if the job somehow reached "completed" with no bytes,
    /// which would be a backend bug.
    pub size_bytes: Option<u64>,

    /// What the user picked in step 4.
    pub context_length: u32,
}
```

### 5.3 Internal state machine (unchanged from today)

```
RepoInput → LoadingQuants → SelectQuants → SetContext → Downloading → Done
```

### 5.4 Behavioral changes vs. today's `pages/pull.rs`

1. **On mount and on (closed → open) transition**, an `Effect` checks
   `(is_open == true) && (initial_repo.get().is_some_non_empty())`. If both
   are true *and* `wizard_step == RepoInput` *or* `wizard_step == Done`, the
   wizard:
   - Resets `selected_filenames`, `context_lengths`, `download_jobs`,
     `error_msg`, and `did_complete` to fresh values.
   - Sets `repo_id` from `initial_repo`.
   - Transitions to `LoadingQuants`.
   - Fires the same `GET /koji/v1/hf/*repo_id` fetch the "Search" button does today.

   The Effect does **not** trigger when `initial_repo` changes while the modal
   is closed (because `is_open == false`), and does **not** trigger when the
   wizard is mid-flow (steps `LoadingQuants` through `Downloading`). This prevents
   background refetches and protects in-progress sessions.

2. **The step indicator hides the "1. Repo" pill** when `initial_repo` is
   non-empty (renders 5 steps instead of 6).

3. **The "Back" button on `SelectQuants` is hidden** when `initial_repo` is
   non-empty (there's nothing to go back to).

4. **A "Cancel" button** appears on steps `RepoInput`, `SelectQuants`, and
   `SetContext` when `on_close` is set, placed to the left of `Back`/`Next` in
   the form-actions row. On the `Downloading` step, a **"Hide"** button appears
   in the same position; clicking it calls `on_close` but downloads keep running
   in the background (this is the whole point of decision #6). On `Done`, the
   "View Models →" link is replaced with a **"Close"** button when `on_complete`
   is set (see point 6 below).

5. **`on_complete` is fired via a dedicated `Effect`** that watches
   `wizard_step`. When the step transitions to `Done` and the internal
   `did_complete: RwSignal<bool>` is `false`:
   - Build the `Vec<CompletedQuant>` from `download_jobs` filtered to
     `status == "completed"`, joined with `available_quants` (for the `quant`
     label) and `context_lengths` (for the user's chosen context).
   - Use `JobProgress.bytes_downloaded` for `size_bytes`.
   - Run the callback.
   - Set `did_complete` to true.

   This guard ensures the callback fires exactly once per session and never on
   spurious re-renders.

6. **The `Done`-step rendering** branches on `on_complete.is_some()`:
   - When set (modal host): render a "Close" button that calls `on_close`.
   - When unset (`/pull` page): render the existing "View Models →" link
     unchanged.

7. **The wizard does NOT render `form-card` chrome.** It renders the
   `wizard-steps` indicator and the per-step body, but the host page is
   responsible for any card/border wrapping. This avoids nested `.form-card`
   when hosted in a modal, and lets `Pull()` apply its own card chrome.

### 5.5 Quant-key derivation (matches backend)

To keep the UI key consistent with what the backend writes to the model card
(`_setup_model_after_pull_with_config` at `crates/koji-core/src/proxy/koji_handlers.rs:893-897`),
the wizard module includes a local copy of `infer_quant_from_filename` with the
same pattern list as `crates/koji-core/src/models/pull.rs:283`.

```rust
/// MUST stay in sync with `infer_quant_from_filename` in
/// `crates/koji-core/src/models/pull.rs`. Duplicated here because `koji-core`
/// is only available under the `ssr` feature and pulls in tokio/sqlite/reqwest
/// which can't compile to WASM.
///
/// If `koji-core` is later split into a WASM-compatible utility crate, replace
/// this with a direct import.
fn infer_quant_from_filename(filename: &str) -> Option<String> {
    // ... same patterns, same matching ...
}
```

The `CompletedQuant.quant` field is built as:

```rust
let quant = available_entry.quant.clone()  // from HF listing
    .or_else(|| infer_quant_from_filename(&filename));
```

And the editor's merge derives the row key as:

```rust
let key = cq.quant.clone()
    .unwrap_or_else(|| cq.filename.trim_end_matches(".gguf").to_string());
```

This three-step fallback (`available_entry.quant` → `infer_quant_from_filename`
→ trimmed filename) matches the backend's chain exactly and prevents the latent
desync the reviewer flagged.

### 5.6 State that stays internal to the wizard

Reactive signals:
- `wizard_step: RwSignal<WizardStep>`
- `repo_id: RwSignal<String>`
- `available_quants: RwSignal<Vec<QuantEntry>>`
- `selected_filenames: RwSignal<HashSet<String>>`
- `context_lengths: RwSignal<HashMap<String, u32>>`
- `download_jobs: RwSignal<Vec<JobProgress>>`
- `error_msg: RwSignal<Option<String>>`
- `did_complete: RwSignal<bool>` *(new — see 5.4 #5)*

None leak via props.

### 5.7 Files-to-move inventory (from `pages/pull.rs`)

Verbatim move into `components/pull_quant_wizard.rs`:

**Types:** `QuantEntry` (line 9), `JobProgress` (16), `PullJobEntry` (28),
`SsePayload` (35), `WizardStep` (47), `PullRequest` (74), `QuantRequest` (80).

**Free functions:** `format_bytes` (60), `step_class` (84).

**Imports:** `gloo_net::eventsource::futures::EventSource`,
`gloo_net::http::Request`, `futures_util::StreamExt` (and `pin_mut`, `select`,
`Either`), `wasm_bindgen_futures::spawn_local`, `serde_json`, `serde::{Deserialize, Serialize}`,
`std::collections::{HashMap, HashSet}`, `leptos::prelude::*`.

**New additions in the wizard module:**
- `pub struct CompletedQuant` (5.2)
- `fn infer_quant_from_filename` (5.5)
- The reset Effect, the `on_complete` Effect, and the `did_complete` signal.
- The Cancel/Hide/Close button rendering blocks (5.4 #4, #6).

### 5.8 Things the wizard does NOT do

- It does not own the modal chrome.
- It does not render `form-card` (host's responsibility).
- It does not know whether it's hosted on `/pull` or in the model editor.
- It does not refresh any list — purely reports completed quants via `on_complete`.
- It does not block close — even with downloads in flight, `on_close` is honored
  and downloads continue in the background.

## 6. The `Modal` shell component

Location: `crates/koji-web/src/components/modal.rs`.

This is the first modal in the codebase. Per decision #6, the modal **always
renders its children** and toggles visibility via a CSS class, so children are
preserved across open/close cycles.

### 6.1 Props

```rust
#[component]
pub fn Modal(
    /// Controls visibility. The modal toggles a CSS class but always renders
    /// its children, so child component state is preserved across open/close.
    #[prop(into)] open: Signal<bool>,

    /// Called when the user dismisses via X button, Escape key, or backdrop
    /// click. The host is responsible for setting `open` to false in response.
    #[prop(into)] on_close: Callback<()>,

    /// Title shown in the modal header.
    #[prop(into)] title: String,

    /// Modal body. **Must be `ChildrenFn`** (not `Children`) so it can be
    /// projected into a reactive context that always renders.
    children: ChildrenFn,
) -> impl IntoView
```

### 6.2 DOM structure

The modal is **always rendered** in the DOM. Visibility is toggled by adding
or removing the `modal-backdrop--open` class on the outer `<div>`:

```html
<div class:modal-backdrop=true class:modal-backdrop--open=open on:click=close>
  <div class="modal" on:click=stop_propagation>
    <div class="modal-header">
      <h2 class="modal-title">{title}</h2>
      <button class="modal-close" on:click=close aria-label="Close">"✕"</button>
    </div>
    <div class="modal-body">
      {children()}
    </div>
  </div>
</div>
```

`children()` is invoked exactly once during component construction (which is
why `ChildrenFn` is required — it permits this single invocation under a
reactive owner that survives the lifetime of the modal). The resulting view is
mounted unconditionally; the `modal-backdrop--open` class controls
`display: flex` vs `display: none`.

### 6.3 Behaviors

- **Backdrop click** → calls `on_close`. The inner `.modal` stops propagation
  so clicks inside don't dismiss.
- **Escape key** → registered **once during component setup** (not inside an
  Effect, to avoid duplicate listeners across re-runs):

  ```rust
  let closure = Closure::wrap(Box::new(move |e: KeyboardEvent| {
      if e.key() == "Escape" && open.get_untracked() {
          on_close.run(());
      }
  }) as Box<dyn Fn(_)>);
  let window = web_sys::window().expect("window");
  window.add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref())
      .expect("add keydown listener");
  on_cleanup(move || {
      let _ = window.remove_event_listener_with_callback(
          "keydown", closure.as_ref().unchecked_ref());
      drop(closure);
  });
  ```

  Note `get_untracked()` — we don't want the listener to re-register every
  time `open` changes.

- **No focus trap, no scroll lock, no `aria-modal`/`role=dialog` wiring.**
  Explicitly YAGNI for now. `aria-label` on the close button is the only
  accessibility nicety.
- **No portal.** Renders in place. The CSS positions it `fixed` over the
  viewport, so DOM location doesn't matter.

### 6.4 CSS additions to `style.css`

```css
.modal-backdrop {
  position: fixed; inset: 0;
  background: rgba(0, 0, 0, 0.6);
  display: none;
  align-items: center; justify-content: center;
  z-index: 1000;
}
.modal-backdrop--open { display: flex; }

.modal {
  background: var(--bg-secondary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  max-width: 720px; width: 90vw;
  max-height: 90vh; overflow-y: auto;
  box-shadow: var(--shadow-card);
}
.modal-header {
  display: flex; align-items: center; justify-content: space-between;
  padding: 1rem 1.5rem;
  border-bottom: 1px solid var(--border-color);
}
.modal-title { margin: 0; font-size: 1.25rem; color: var(--text-primary); }
.modal-close {
  background: none; border: none;
  color: var(--text-secondary);
  font-size: 1.25rem; cursor: pointer;
  padding: 0.25rem 0.5rem;
}
.modal-close:hover { color: var(--text-primary); }
.modal-body { padding: 1.5rem; }
```

CSS variable names are taken from `crates/koji-web/style.css:9-32`
(`--bg-secondary`, `--border-color`, `--radius-md`, `--shadow-card`, etc.) so
the modal matches the existing dark theme automatically.

## 7. `pages/pull.rs` becomes a thin wrapper

After the wizard is extracted, `pages/pull.rs` shrinks dramatically. It keeps
only the page header and the form-card chrome (since the wizard no longer
renders its own — see 5.4 #7), and mounts the wizard with no pre-set repo:

```rust
use crate::components::pull_quant_wizard::PullQuantWizard;
use leptos::prelude::*;

#[component]
pub fn Pull() -> impl IntoView {
    view! {
        <div class="page-header">
            <h1>"Pull Model"</h1>
        </div>
        <div class="form-card card">
            <PullQuantWizard
                initial_repo=Signal::derive(|| String::new())
                // is_open omitted (defaults to None → wizard treats as always open)
                // on_complete omitted → Done step shows "View Models →" link
                // on_close omitted → no Cancel buttons rendered
            />
        </div>
    }
}
```

When all three optional props are unset, the wizard renders exactly as the
current `/pull` page does today — same step indicator (with "1. Repo" pill),
same buttons, same `Done`-step link. This is what makes the refactor a true
zero-behavior-change for the existing `/pull` page.

**Verification step:** before touching the model editor at all, do the
extract-and-wrap refactor and confirm `/pull` still works end-to-end (manual
smoke test 1). Only then proceed to section 8.

## 8. Model editor changes

In `crates/koji-web/src/pages/model_editor.rs`:

### 8.1 New signal for modal visibility

Alongside the existing form signals near line 281:

```rust
let pull_modal_open = RwSignal::new(false);
// add_quant_counter is removed — no longer needed
```

### 8.2 Replace the `+ Add Quant` button

Currently around lines 1170-1180. Replace with a `+ Pull Quant` button. To
work around browsers (Firefox in particular) ignoring `title` on disabled
form controls, wrap the button in a `<span>` that carries the tooltip:

```rust
<div class="mt-1">
    <span title=move || if form_model.get().trim().is_empty() {
        "Enter the HuggingFace repo above before pulling quants"
    } else {
        "Pull a new quant from HuggingFace"
    }>
        <button
            type="button"
            class="btn btn-primary btn-sm"
            prop:disabled=move || form_model.get().trim().is_empty()
            on:click=move |_| pull_modal_open.set(true)
        >"+ Pull Quant"</button>
    </span>
</div>
```

The button is `btn-primary` (not `btn-secondary` like the old button) to
signal it's the recommended action.

### 8.3 Mount the modal + wizard

At the bottom of the model editor's main view, after the form:

```rust
<Modal
    open=pull_modal_open.into()
    on_close=Callback::new(move |_| pull_modal_open.set(false))
    title="Pull Quant from HuggingFace".to_string()
>
    <PullQuantWizard
        initial_repo=Signal::derive(move || form_model.get())
        is_open=pull_modal_open.into()
        on_complete=Callback::new(move |completed: Vec<CompletedQuant>| {
            quants.update(|rows| {
                for cq in completed {
                    let key = cq.quant.clone()
                        .unwrap_or_else(|| cq.filename.trim_end_matches(".gguf").to_string());
                    if let Some(pos) = rows.iter().position(|(k, _)| k == &key) {
                        // Re-pull: overwrite filename and context (user just
                        // picked these in the wizard, so they're the latest
                        // intent). Only overwrite size_bytes if we got a value
                        // — never clobber a known size with None.
                        let row = &mut rows[pos].1;
                        row.file = cq.filename;
                        row.context_length = Some(cq.context_length);
                        if cq.size_bytes.is_some() {
                            row.size_bytes = cq.size_bytes;
                        }
                    } else {
                        // New row.
                        rows.push((key, QuantInfo {
                            file: cq.filename,
                            size_bytes: cq.size_bytes,
                            context_length: Some(cq.context_length),
                        }));
                    }
                }
            });
            pull_modal_open.set(false);
        })
        on_close=Callback::new(move |_| pull_modal_open.set(false))
    />
</Modal>
```

### 8.4 Key behaviors

- `initial_repo` is `Signal::derive` over `form_model`. Combined with
  `is_open`, the wizard's reset Effect (5.4 #1) only refetches when the modal
  actually opens, never reactively in the background while closed.
- `on_complete` does the **field-by-field merge**:
  - **Quant key derivation** matches the backend's logic (see 5.5).
  - **Upsert semantics on re-pull**: overwrite `file` and `context_length`
    (the wizard's values reflect the user's latest intent — they just clicked
    through to set them). Only overwrite `size_bytes` when the wizard supplies
    a non-None value, so the merge can never clobber a correct on-disk size
    with `None`.
  - Modal closes automatically on completion. Because the Modal preserves
    children, the wizard sits in the `Done` state in the background; on next
    open the reset Effect (5.4 #1) clears it back to `LoadingQuants`.
- **Mid-download close is safe**: per decision #6 the wizard's SSE futures
  keep running. If the user closes the modal mid-download, downloads continue,
  `on_complete` still fires when the last job finishes, and the merge happens
  even though the user can't see the progress UI. The user will see the new
  rows in the editor's quants table on next render of that area (the signal
  update is reactive). To make this less surprising we'll log a console
  message ("Pull-quant downloads continuing in background"); a future
  iteration could add a toast.

### 8.5 Remove unused state

The `add_quant_counter` signal and its only usage disappear. No other places
in the file reference it.

### 8.6 Save round-trip integrity

The Save button continues to serialize `quants` exactly as it does today. Three
properties guarantee consistency between UI and on-disk state:

1. The wizard's quant key matches the backend's quant key (5.5).
2. The merge never clobbers a valid `size_bytes` with `None` (8.4).
3. The backend's `setup_model_after_pull` writes to disk independently of the
   client, so even if the user navigates away mid-pull and never sees the
   merge, the model card on disk is correct on next page load.

### 8.7 Documented caveats

- **Cancel does not undo a pull.** If the user pulls a quant and then *cancels
  the form*, the new quant rows are still on disk in the model card (the
  backend wrote them at pull time). They will reappear next time the page is
  loaded. Consistent with how `/pull` behaves today.

- **Failed quants are dropped from the merge.** If the user pulls three quants
  and one fails, the two successful ones are merged and the third is silently
  omitted. The user sees the failure in the wizard's `Done` step, but once
  they click Close that information is gone. Acceptable for v1; a future
  iteration could surface failures in a toast or log.

- **Mid-download close is silent.** When the modal is closed mid-download, the
  user has no visible indication that downloads are still running. We log a
  message to the browser console; a toast is a follow-up.

## 9. Testing strategy

### Automated

- `cargo build --workspace` — must compile cleanly.
- `cargo clippy --workspace -- -D warnings` — must pass with zero warnings
  (per AGENTS.md).
- `cargo fmt --all` — formatting clean.
- `cargo test --workspace` — all existing tests must still pass.

No new automated tests are added, because:

- The wizard's logic is being **moved verbatim**, not reimplemented. Behavior
  preservation is verified by manual smoke testing the existing `/pull` page.
- The model editor's merge logic is small (~20 lines) and tightly coupled to
  Leptos signals, which are awkward to test outside a browser.

### Manual smoke tests (acceptance criteria)

**1. `/pull` regression** — performed *after* section 7, *before* section 8:
- Visit `/pull`, enter a real HF repo (with `-GGUF` suffix), search → list appears.
- Select a quant, set context, start download → progress bar updates via SSE.
- Reach `Done` step → "View Models →" link works.
- Confirms the extracted wizard is byte-equivalent in behavior to the old page.

**2. Model editor — disabled state:**
- Visit `/models/new`. The `+ Pull Quant` button is disabled. Hovering shows
  the tooltip on the wrapping span.
- Type a repo into the model field. The button enables.

**3. Model editor — happy path:**
- Visit an existing model with a populated `model` field (use a `-GGUF` repo).
- Click `+ Pull Quant` → modal opens, wizard immediately fetches HF quants
  (skips step 1, no "1. Repo" pill in the indicator).
- Pick one quant, set context, download.
- On `Done`, click "Close" → modal closes, the new quant row is visible in
  the editor's quants table with correct `file`, `size_bytes`, and
  `context_length`.
- Click Save → no errors, model card on disk reflects the new quant. Reload
  the page and confirm the row is still there with the same values.

**4. Model editor — re-pull (upsert):**
- Open the same modal again, select the same quant, change context length,
  complete download.
- The existing row in the table is updated in place (not duplicated).
- `file`, `size_bytes`, and `context_length` reflect the new values.

**5. Model editor — cancel paths:**
- Open the modal, click X → closes, no quant rows added.
- Open the modal, press Escape → closes.
- Open the modal, click backdrop → closes.
- Open the modal, click in-step Cancel button → closes.

**6. Model editor — preserves unsaved edits:**
- Open an existing model, type a new value into "Display name" (don't save).
- Pull a quant via the modal.
- After completion, the display-name edit is still in the field (we did not
  re-fetch).

**7. Model editor — mid-download close (decision #6):**
- Open the modal, start a download of a reasonably large quant.
- Before it finishes, close the modal via X.
- Reopen the modal — the wizard should reset to `LoadingQuants` and refetch
  (because it was no longer in mid-flow per the reset Effect's gating? **No**
  — the reset only fires when previous step is `RepoInput` or `Done`.
  Mid-flow sessions are preserved). Verify: reopening shows the still-running
  Downloading step with progress.
- Wait for completion. The merge fires while the modal is open or closed,
  whichever the user happens to be in. The new row appears in the quants table.

**8. Model editor — modal-closed completion:**
- Open the modal, start a download.
- Close the modal mid-download.
- Wait long enough for the download to complete in the background.
- Without reopening the modal, observe the editor's quants table — the new
  row should be present (the merge happened via `on_complete` while the modal
  was hidden).

## 10. File-by-file change list

| File | Change | Approx LOC |
|---|---|---|
| `crates/koji-web/src/components/mod.rs` | Add `pub mod modal; pub mod pull_quant_wizard;` | +2 |
| `crates/koji-web/src/components/modal.rs` | **New.** `Modal` component with `ChildrenFn`, Escape listener, CSS-toggle visibility | ~100 |
| `crates/koji-web/src/components/pull_quant_wizard.rs` | **New.** `PullQuantWizard` component, `CompletedQuant` type, local `infer_quant_from_filename`, reset Effect, completion Effect | ~700 |
| `crates/koji-web/src/pages/pull.rs` | **Shrink to thin wrapper** | ~20 (was ~600) |
| `crates/koji-web/src/pages/model_editor.rs` | Remove `add_quant_counter`. Add `pull_modal_open`. Replace button (with span wrapper for tooltip). Mount modal + wizard with field-by-field merge. | +60, -10 |
| `crates/koji-web/style.css` | Add modal overlay styles using existing CSS variables | +35 |

**Net total:** ~+925 lines, ~-610 lines (mostly the move of the wizard).

## 11. Implementation order

1. **Refactor only.** Create `modal.rs` (unused for now, just compiles), create
   `pull_quant_wizard.rs` with the hoisted state machine + new additions
   (CompletedQuant, infer_quant_from_filename, reset Effect, completion Effect,
   Cancel buttons), shrink `pages/pull.rs`. → `cargo build` + manual smoke
   test 1 (`/pull` regression).

2. **Wire the new feature.** Modify `model_editor.rs`, add CSS.
   → `cargo build` + smoke tests 2–8.

3. **Lint pass.** `cargo clippy --workspace -- -D warnings`, `cargo fmt --all`,
   `cargo test --workspace`.

4. **Commit per logical step**, push branch, open PR to `develop`.

## 12. Out-of-scope notes / pre-existing issues

These are real but explicitly **not** addressed by this PR:

- **`-GGUF` auto-resolution gap.** `handle_hf_list_quants` calls
  `fetch_blob_metadata` directly, which does not auto-append `-GGUF` to repo
  IDs the way `list_gguf_files` does. As a result, a user with `form_model =
  "bartowski/Qwen3-8B"` (no suffix) will hit `/koji/v1/hf/bartowski/Qwen3-8B`,
  get an empty/error response, and be dropped into the wizard's error state.
  This bug also exists on `/pull`, but is more visible from the model editor
  button because users may not realize the suffix matters. **Follow-up:** make
  `handle_hf_list_quants` perform the same `-GGUF` resolution as
  `list_gguf_files` (or factor out a shared resolver).

- **Wizard component is monolithic** (~700 LOC). AGENTS.md prefers small
  focused functions. Splitting per-step into sub-components is a worthwhile
  follow-up but out of scope here — the move-as-is approach minimizes refactor
  risk and lets the `/pull` regression test catch any behavior drift.

- **No focus trap / scroll lock** on the modal. First modal in the codebase;
  if more modals appear, revisit accessibility wholesale.

- **Failed-pull surfacing.** Failed quants are silently dropped from the merge
  after the user dismisses the `Done` step. Future: toast or persistent log.

- **Mid-download close has no visible indicator.** Console log only. Future: toast.
