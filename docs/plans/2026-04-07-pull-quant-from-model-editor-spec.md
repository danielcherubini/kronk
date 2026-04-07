# Pull Quant from HuggingFace on the Model Editor — Spec

**Status:** Draft
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
    (`crates/koji-core/src/proxy/koji_handlers.rs::handle_hf_list_quants`)
  - `POST /koji/v1/pulls` — starts download jobs
  - `GET /koji/v1/pulls/:job_id/stream` — SSE progress stream
- On success, the backend handler `setup_model_after_pull`
  (`crates/koji-core/src/proxy/koji_handlers.rs`) writes/updates the model card
  on disk **and** inserts/updates the entry into the live in-memory config.

### Key insight

The backend already does the right thing. When a pull completes, the new quants
are merged into the existing model card and live config. **No backend changes
are required.** This is purely a Leptos client-side refactor + UI feature.

## 3. Design decisions (recorded from brainstorming)

| # | Decision | Choice |
|---|---|---|
| 1 | Where the wizard lives | Extract into a reusable Leptos component, embed in both `/pull` and the model editor |
| 2 | How it appears in the model editor | Modal overlay (dim background) |
| 3 | Whether to filter already-pulled quants | Show all, allow re-pull (matches current `/pull` behavior; backend upserts) |
| 4 | What happens after download completes | Client-side merge of completed quants into the local `quants` signal (preserves unsaved form edits) |
| 5 | What the button does when `form_model` is empty | Disabled, with a tooltip |

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

### Props

```rust
#[component]
pub fn PullQuantWizard(
    /// Pre-set HF repo ID. If non-empty, the wizard skips step 1 and immediately
    /// fetches quants. If empty, the wizard starts at the repo-input step.
    #[prop(into)] initial_repo: Signal<String>,

    /// Called once after ALL downloads in this session have reached a terminal
    /// state (Completed or Failed). Receives the list of quants that completed
    /// successfully so the host can merge them into its own state.
    /// Not called if the user cancels before any downloads finish.
    #[prop(optional)] on_complete: Option<Callback<Vec<CompletedQuant>>>,

    /// Called when the user dismisses the wizard (Cancel / X / backdrop click).
    /// The host decides whether to close the modal — wizard never unmounts itself.
    #[prop(optional)] on_close: Option<Callback<()>>,
) -> impl IntoView
```

### Public type emitted on completion

```rust
#[derive(Clone, Debug)]
pub struct CompletedQuant {
    pub repo_id: String,        // the resolved repo (may have -GGUF appended by backend)
    pub filename: String,       // e.g. "Qwen3-8B-Q4_K_M.gguf"
    pub quant: Option<String>,  // e.g. "Q4_K_M" — inferred from filename by backend
    pub size_bytes: Option<u64>,// from the HF listing (not from the download)
    pub context_length: u32,    // what the user picked in step 4
}
```

### Internal state machine (unchanged from today)

```
RepoInput → LoadingQuants → SelectQuants → SetContext → Downloading → Done
```

### Behavioral changes vs. today's `pages/pull.rs`

1. On mount, an `Effect` reads `initial_repo`. If non-empty, it sets `repo_id`
   and immediately transitions to `LoadingQuants`, kicking off the same
   `GET /koji/v1/hf/*repo_id` fetch the "Search" button does today. The user
   never sees step 1.
2. The step indicator hides the "1. Repo" pill when `initial_repo` is non-empty
   (we render 5 steps instead of 6).
3. The "Back" button on `SelectQuants` is hidden when `initial_repo` is
   non-empty (there's nothing to go back to).
4. When the wizard reaches `Done`, it calls `on_complete` once with the list of
   `CompletedQuant`s assembled from the wizard's known state (selected filenames
   + context_lengths + quant entries from `available_quants`), filtered to
   those whose final `JobProgress.status == "completed"`.
5. The "View Models →" link on the `Done` step is replaced with a "Close" button
   that calls `on_close` instead of navigating, **when `on_complete` is set**
   (the host indicates it wants to handle completion). When `on_complete` is
   not set (the `/pull` page case), the existing "View Models →" link is rendered
   unchanged.

### State that stays internal to the wizard

`wizard_step`, `repo_id`, `available_quants`, `selected_filenames`,
`context_lengths`, `download_jobs`, `error_msg`. None of this leaks via props.

### Things the wizard does NOT do

- It does not own the modal chrome.
- It does not know whether it's hosted on `/pull` or in the model editor.
- It does not refresh any list — purely reports completed quants via `on_complete`.

## 6. The `Modal` shell component

Location: `crates/koji-web/src/components/modal.rs`.

This is the first modal in the codebase. Kept minimal and reusable rather than
over-designed.

### Props

```rust
#[component]
pub fn Modal(
    /// Controls visibility. When false, the component renders nothing.
    #[prop(into)] open: Signal<bool>,

    /// Called when the user dismisses via X button, Escape key, or backdrop click.
    /// The host is responsible for setting `open` to false in response.
    #[prop(into)] on_close: Callback<()>,

    /// Title shown in the modal header.
    #[prop(into)] title: String,

    /// Modal body — typically a wizard or form.
    children: Children,
) -> impl IntoView
```

### DOM structure (rendered only when `open.get() == true`)

```html
<div class="modal-backdrop" on:click=close>
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

### Behaviors

- **Backdrop click** → calls `on_close`. The inner `.modal` stops propagation
  so clicks inside don't dismiss.
- **Escape key** → a `window.addEventListener("keydown")` registered in an
  `Effect` (cleaned up on unmount via `on_cleanup`) calls `on_close` when
  Escape is pressed and `open` is true.
- **No focus trap, no scroll lock, no aria-modal/role=dialog wiring.**
  Explicitly YAGNI for now — if the codebase grows more modals we can
  revisit. Adding `aria-label` on the close button is the only accessibility
  nicety.
- **No portal.** Renders in place. The CSS positions it `fixed` over the
  viewport, so DOM location doesn't matter.

### CSS additions to `style.css`

```css
.modal-backdrop {
  position: fixed; inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex; align-items: center; justify-content: center;
  z-index: 1000;
}
.modal {
  background: var(--bg-card, #fff);
  border-radius: 8px;
  max-width: 720px; width: 90vw;
  max-height: 90vh; overflow-y: auto;
  box-shadow: 0 10px 40px rgba(0, 0, 0, 0.3);
}
.modal-header {
  display: flex; align-items: center; justify-content: space-between;
  padding: 1rem 1.5rem;
  border-bottom: 1px solid var(--border, #e5e5e5);
}
.modal-title { margin: 0; font-size: 1.25rem; }
.modal-close {
  background: none; border: none; font-size: 1.25rem; cursor: pointer;
  padding: 0.25rem 0.5rem;
}
.modal-body { padding: 1.5rem; }
```

Exact CSS variable names / colors will be aligned with the existing theme
when implementing — this is the structural intent.

## 7. `pages/pull.rs` becomes a thin wrapper

After the wizard is extracted, `pages/pull.rs` shrinks dramatically. It keeps
only the page chrome and mounts the wizard with no pre-set repo:

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
            />
        </div>
    }
}
```

That's the entire file. No `on_complete` is passed (so the wizard's `Done` step
shows the existing "View Models →" link), no `on_close` is passed (so the X
button isn't relevant — the wizard isn't in a modal, it's a full page).

**Implication for the wizard:** when neither callback is set, the wizard
renders exactly as the current `/pull` page does today — same step indicator,
same buttons, same `Done`-step link. This is what makes the refactor a true
zero-behavior-change for the existing `/pull` page.

**Verification step:** before touching the model editor at all, do the
extract-and-wrap refactor and confirm `/pull` still works end-to-end (manual
smoke test). Only then proceed to section 8. This keeps the refactor safely
separable from the new feature.

## 8. Model editor changes

In `crates/koji-web/src/pages/model_editor.rs`:

### 8a. New signal for modal visibility

Alongside the existing form signals near line 281:

```rust
let pull_modal_open = RwSignal::new(false);
// add_quant_counter is removed — no longer needed
```

### 8b. Replace the `+ Add Quant` button

Currently around line 1170-1180. Replace with a `+ Pull Quant` button:

```rust
<div class="mt-1">
    <button
        type="button"
        class="btn btn-primary btn-sm"
        prop:disabled=move || form_model.get().trim().is_empty()
        title=move || if form_model.get().trim().is_empty() {
            "Enter the HuggingFace repo above before pulling quants"
        } else {
            "Pull a new quant from HuggingFace"
        }
        on:click=move |_| pull_modal_open.set(true)
    >"+ Pull Quant"</button>
</div>
```

The button is `btn-primary` (not `btn-secondary` like the old button) to
signal it's the recommended action. The `title` attribute is dynamic so
screen readers and hover tooltips reflect the current state.

### 8c. Mount the modal + wizard

At the bottom of the model editor's main view, after the form:

```rust
<Modal
    open=pull_modal_open.into()
    on_close=Callback::new(move |_| pull_modal_open.set(false))
    title="Pull Quant from HuggingFace".to_string()
>
    <PullQuantWizard
        initial_repo=Signal::derive(move || form_model.get())
        on_complete=Callback::new(move |completed: Vec<CompletedQuant>| {
            quants.update(|rows| {
                for cq in completed {
                    let key = cq.quant.clone()
                        .unwrap_or_else(|| cq.filename.trim_end_matches(".gguf").to_string());
                    let info = QuantInfo {
                        file: cq.filename,
                        size_bytes: cq.size_bytes,
                        context_length: Some(cq.context_length),
                    };
                    if let Some(pos) = rows.iter().position(|(k, _)| k == &key) {
                        rows[pos].1 = info;  // upsert
                    } else {
                        rows.push((key, info));
                    }
                }
            });
            pull_modal_open.set(false);
        })
        on_close=Callback::new(move |_| pull_modal_open.set(false))
    />
</Modal>
```

### Key behaviors

- `initial_repo` is `Signal::derive` over `form_model`, so if the user edits
  the repo field and re-opens the modal, the wizard reads the latest value.
- `on_complete` does the **client-side merge** decided in question 4:
  - **Quant key derivation** matches the backend's logic in
    `_setup_model_after_pull_with_config`: prefer `cq.quant`, fall back to
    `filename` minus `.gguf`. This keeps the UI key consistent with what the
    backend writes to the model card.
  - **Upsert semantics:** if a row with the same key already exists, replace
    it (covers the re-pull case from question 3); otherwise append.
  - Modal closes automatically on completion. The user is left looking at an
    updated quants table within the still-unsaved form.
- `on_close` (called from X / Escape / backdrop / wizard's "Cancel") just
  closes the modal without merging.

### 8d. Remove unused state

The `add_quant_counter` signal and its only usage disappear. No other places in
the file reference it.

### 8e. No changes to the rest of the form

The Save button continues to serialize `quants` exactly as it does today.
Because the backend already wrote the new quants to the model card on disk
during the pull, the Save round-trip is consistent: the UI state and the file
state agree on the new quant rows.

### Documented caveat

If the user pulls a quant and then *cancels the form* (instead of saving), the
new quant rows are still on disk in the model card (the backend wrote them at
pull time). They will reappear next time the page is loaded. This is consistent
with how `/pull` behaves today, and arguably correct — a download is a side
effect that can't be "undone" by canceling a form. We document it but do not
try to fix it.

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
- The model editor's merge logic is small (~15 lines) and tightly coupled to
  Leptos signals, which are awkward to test outside a browser.

### Manual smoke tests (acceptance criteria)

**1. `/pull` regression** — performed *after* section 7, *before* section 8:
- Visit `/pull`, enter a real HF repo, search → list appears.
- Select a quant, set context, start download → progress bar updates via SSE.
- Reach `Done` step → "View Models →" link works.
- Confirms the extracted wizard is byte-equivalent in behavior to the old page.

**2. Model editor — disabled state:**
- Visit `/models/new`. The `+ Pull Quant` button is disabled. Hovering shows
  the tooltip.
- Type a repo into the model field. The button enables.

**3. Model editor — happy path:**
- Visit an existing model with a populated `model` field.
- Click `+ Pull Quant` → modal opens, wizard immediately fetches HF quants
  (skips step 1, no "1. Repo" pill in the indicator).
- Pick one quant, set context, download.
- On `Done`, click "Close" → modal closes, the new quant row is visible in
  the editor's quants table.
- Click Save → no errors, model card on disk reflects the new quant.

**4. Model editor — re-pull (upsert):**
- Open the same modal again, select the same quant, change context length,
  complete download.
- The existing row in the table is updated in place (not duplicated).

**5. Model editor — cancel paths:**
- Open the modal, click X → closes, no quant rows added.
- Open the modal, press Escape → closes.
- Open the modal, click backdrop → closes.

**6. Model editor — preserves unsaved edits:**
- Open an existing model, type a new value into "Display name" (don't save).
- Pull a quant via the modal.
- After completion, the display-name edit is still in the field (we did not
  re-fetch).

## 10. File-by-file change list

| File | Change | Approx LOC |
|---|---|---|
| `crates/koji-web/src/components/mod.rs` | Add `pub mod modal; pub mod pull_quant_wizard;` | +2 |
| `crates/koji-web/src/components/modal.rs` | **New.** `Modal` component | ~80 |
| `crates/koji-web/src/components/pull_quant_wizard.rs` | **New.** `PullQuantWizard` component + `CompletedQuant` type. Hoists ~600 lines of state machine from `pages/pull.rs`. | ~620 |
| `crates/koji-web/src/pages/pull.rs` | **Shrink to thin wrapper** | ~15 (was ~600) |
| `crates/koji-web/src/pages/model_editor.rs` | Remove `add_quant_counter`. Add `pull_modal_open`. Replace button. Mount modal + wizard. | +50, -10 |
| `crates/koji-web/style.css` | Add modal overlay styles | +30 |

**Net total:** ~+800 lines, ~-610 lines (mostly the move of the wizard).

## 11. Implementation order

1. **Refactor only.** Create `modal.rs` (unused for now, just compiles), create
   `pull_quant_wizard.rs` with the hoisted state machine, shrink `pages/pull.rs`.
   → `cargo build` + manual smoke test 1 (`/pull` regression).
2. **Wire the new feature.** Modify `model_editor.rs`, add CSS.
   → `cargo build` + smoke tests 2–6.
3. **Lint pass.** `cargo clippy --workspace -- -D warnings`, `cargo fmt --all`,
   `cargo test --workspace`.
4. **Commit per logical step**, push branch, open PR to `develop`.
