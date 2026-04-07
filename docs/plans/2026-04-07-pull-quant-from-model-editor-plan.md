# Pull Quant from HuggingFace on Model Editor — Implementation Plan

**Goal:** Replace the bare "+ Add Quant" button on the koji model edit page with a "+ Pull Quant" button that opens a modal-hosted wizard reused from the existing `/pull` page, downloading new GGUF quants from HuggingFace and merging them into the editor's quants table without losing unsaved form edits.

**Architecture:** Extract the existing wizard state machine from `crates/koji-web/src/pages/pull.rs` into a reusable Leptos component (`components/pull_quant_wizard.rs`). Introduce a new general-purpose `Modal` shell component (`components/modal.rs`) that renders children unconditionally and toggles visibility via a CSS class, so the wizard's SSE futures keep running across modal close/reopen cycles. The model editor mounts the wizard inside the modal, with `form_model` pre-filled as `initial_repo`, and merges completed quants into its local `quants` signal via an `on_complete` callback.

**Tech Stack:** Rust + Leptos 0.7 (CSR/WASM), `gloo-net` for HTTP + SSE, `web-sys` for keyboard events, existing koji backend endpoints (`GET /koji/v1/hf/*repo_id`, `POST /koji/v1/pulls`, `GET /koji/v1/pulls/:job_id/stream`) — no backend changes.

**Reference spec:** [`docs/plans/2026-04-07-pull-quant-from-model-editor-spec.md`](./2026-04-07-pull-quant-from-model-editor-spec.md) — read this first. Every task in this plan corresponds to a section of that spec; section numbers are cited inline.

**Branch:** `feature/pull-quant-from-model-editor` (already created, spec already committed).

**Project conventions** (from `AGENTS.md` and `Makefile`):
- Format: `cargo fmt --all`
- Lint: `cargo clippy --workspace -- -D warnings` (the project's full lint pass also runs `cargo clippy --package koji-web --features ssr -- -D warnings` — use `make clippy` to run both)
- Test: `cargo test --workspace` (or `make test`, which additionally rebuilds the WASM frontend via `trunk build` and runs `cargo test --package koji-web --features ssr`)
- Build (host crates only): `cargo build --workspace`
- Build the WASM frontend (required to actually serve the UI): `make build-frontend-dev`
- Run a local koji instance for smoke tests: `make build-frontend-dev && cargo run -p koji-cli -- serve` — then open `http://127.0.0.1:11434/pull` (or `/models`, etc.) in a browser. The default port is 11434 (`crates/koji-cli/src/cli.rs:120`).
- Commit prefixes: `feat:`, `fix:`, `chore:`, `docs:`, `refactor:`
- 4-space indent, max line ~100 chars
- Doc comments on public items

**Note on smoke tests:** every smoke test in this plan that says "navigate to `/something`" assumes you have started a local koji instance via the command above. The WASM frontend must be rebuilt with `make build-frontend-dev` after any change to a `crates/koji-web` source file before that change is visible in the browser.

---

## Task 1: Add the `Modal` shell component

**Context:**
The koji-web crate currently has zero modal components — `grep -ri modal crates/koji-web/src` returns nothing. Before any wizard work, we introduce a general-purpose `Modal` component that other code can mount inside. Critical design choice (decided in spec rev 3): the modal **always renders its children** in the DOM and toggles visibility via the `modal-backdrop--open` CSS class, so child components are preserved across open/close cycles. This is what makes it safe for the pull wizard to keep its SSE futures running when the user closes the modal mid-download. As a consequence, the `children` prop must be `ChildrenFn` (not `Children`), because Leptos's `Children` is a single-shot `FnOnce`.

The Modal also needs an Escape-key handler. The handler is registered **once** at component setup (not inside a Leptos `Effect`, which would re-register on every dependency change). It uses `wasm_bindgen::closure::Closure` and is cleaned up via `on_cleanup`. This requires `web_sys::KeyboardEvent`, which is **not** currently in the `web-sys` features list of `crates/koji-web/Cargo.toml` — that file must be modified or the build will fail.

This task delivers a compiles-and-runs-but-unused Modal component. No other file references it yet. Spec sections 6.1–6.4 fully describe the props, DOM, behavior, and CSS.

**Files:**
- Create: `crates/koji-web/src/components/modal.rs`
- Modify: `crates/koji-web/src/components/mod.rs` (add `pub mod modal;`)
- Modify: `crates/koji-web/Cargo.toml` (add `KeyboardEvent` to `web-sys` features)
- Modify: `crates/koji-web/style.css` (append modal CSS)

**What to implement:**

1. **`crates/koji-web/Cargo.toml`** — locate the `web-sys` dependency line:
   ```toml
   web-sys = { workspace = true, features = ["Window", "Document", "HtmlElement", "HtmlInputElement", "EventSource", "EventSourceInit", "MessageEvent", "Event"] }
   ```
   Append both `"KeyboardEvent"` (needed by Modal's Escape handler in this task) and `"Console"` (needed by Task 3's `web_sys::console::warn_1` call — colocated here so all `web-sys` feature changes happen in one commit). Final:
   ```toml
   web-sys = { workspace = true, features = ["Window", "Document", "HtmlElement", "HtmlInputElement", "EventSource", "EventSourceInit", "MessageEvent", "Event", "KeyboardEvent", "Console"] }
   ```

   Note: `web_sys::console::warn_1` requires the `Console` feature
   (`web-sys-0.3.x/src/features/gen_console.rs` documents this). No other file
   in koji-web currently uses `web_sys::console::*`, so `"Console"` is genuinely
   new. Without it, Task 3's `cargo build --workspace` will fail with `function
   or associated item warn_1 not found in module console`.

2. **`crates/koji-web/src/components/mod.rs`** — add `pub mod modal;` alongside the existing `pub mod nav;` and `pub mod sparkline;` declarations.

3. **`crates/koji-web/src/components/modal.rs`** — new file. Implement:

   The Closure construction style follows the existing convention in
   `crates/koji-web/src/pages/dashboard.rs` (around line 115): use
   `Closure::<dyn Fn(...)>::new(move |evt| { ... })`, not
   `Closure::wrap(Box::new(...) as Box<dyn Fn(_)>)`. The class-toggle uses the
   tuple form `class=("modal-backdrop--open", move || open.get())` documented at
   <https://docs.rs/leptos/latest/leptos/macro.view.html> point 7 (the bare
   `class:double-hyphen-name` form is unverified through the rstml parser).

   ```rust
   use leptos::prelude::*;
   use wasm_bindgen::closure::Closure;
   use wasm_bindgen::JsCast;
   use web_sys::KeyboardEvent;

   /// A general-purpose modal overlay.
   ///
   /// The modal **always renders** its children in the DOM and toggles visibility
   /// via the `modal-backdrop--open` CSS class. This preserves child component
   /// state (signals, in-flight async work, SSE streams) across open/close
   /// cycles. As a result, `children` must be `ChildrenFn`.
   ///
   /// Dismissal: backdrop click, the X button in the header, and the Escape
   /// key all invoke `on_close`. The host is responsible for setting `open` to
   /// false in response — the modal does not hide itself.
   #[component]
   pub fn Modal(
       /// Whether the modal is currently visible.
       #[prop(into)] open: Signal<bool>,
       /// Called when the user dismisses via X / Escape / backdrop click.
       #[prop(into)] on_close: Callback<()>,
       /// Title shown in the modal header.
       #[prop(into)] title: String,
       /// Modal body. `ChildrenFn` so it can be projected into a reactive
       /// always-rendered tree.
       children: ChildrenFn,
   ) -> impl IntoView {
       // Register a keydown listener once at component setup. NOT in an Effect,
       // because an Effect would re-register on every signal change and leak
       // listeners. Style mirrors `dashboard.rs:115` —
       // `Closure::<dyn Fn(...)>::new(move |evt| { ... })`.
       {
           let closure = Closure::<dyn Fn(KeyboardEvent)>::new(
               move |e: KeyboardEvent| {
                   if e.key() == "Escape" && open.get_untracked() {
                       on_close.run(());
                   }
               },
           );
           let window = web_sys::window().expect("window");
           window
               .add_event_listener_with_callback(
                   "keydown",
                   closure.as_ref().unchecked_ref(),
               )
               .expect("add keydown listener");
           on_cleanup(move || {
               if let Some(window) = web_sys::window() {
                   let _ = window.remove_event_listener_with_callback(
                       "keydown",
                       closure.as_ref().unchecked_ref(),
                   );
               }
               drop(closure);
           });
       }

       // Click handlers.
       let close_cb = on_close;
       let on_backdrop_click = move |_| close_cb.run(());
       let on_modal_click = move |e: leptos::ev::MouseEvent| {
           e.stop_propagation();
       };
       let on_x_click = move |_| close_cb.run(());

       view! {
           <div
               class="modal-backdrop"
               class=("modal-backdrop--open", move || open.get())
               on:click=on_backdrop_click
           >
               <div class="modal" on:click=on_modal_click>
                   <div class="modal-header">
                       <h2 class="modal-title">{title}</h2>
                       <button
                           type="button"
                           class="modal-close"
                           on:click=on_x_click
                           aria-label="Close"
                       >"✕"</button>
                   </div>
                   <div class="modal-body">
                       {children()}
                   </div>
               </div>
           </div>
       }
   }
   ```

   Notes:
   - `children()` is invoked **exactly once** during component construction. The resulting `AnyView` is mounted unconditionally inside `.modal-body`. Visibility is controlled solely by the CSS class on `.modal-backdrop`.
   - Use `get_untracked()` inside the keydown closure so the closure doesn't subscribe to `open`.
   - The `on_cleanup` block removes the listener when the Modal is unmounted (which only happens when its host unmounts, e.g. navigating away from the page).
   - Do **not** add `role="dialog"`, `aria-modal`, focus trap, or scroll lock — explicitly out of scope (spec §6.3).

4. **`crates/koji-web/style.css`** — append at end of file:

   ```css
   /* ── Modal overlay ──────────────────────────────────────────────────────── */
   .modal-backdrop {
       position: fixed;
       inset: 0;
       background: rgba(0, 0, 0, 0.6);
       display: none;
       align-items: center;
       justify-content: center;
       z-index: 1000;
   }
   .modal-backdrop--open {
       display: flex;
   }
   .modal {
       background: var(--bg-secondary);
       border: 1px solid var(--border-color);
       border-radius: var(--radius-md);
       max-width: 720px;
       width: 90vw;
       max-height: 90vh;
       overflow-y: auto;
       box-shadow: var(--shadow-card);
   }
   .modal-header {
       display: flex;
       align-items: center;
       justify-content: space-between;
       padding: 1rem 1.5rem;
       border-bottom: 1px solid var(--border-color);
   }
   .modal-title {
       margin: 0;
       font-size: 1.25rem;
       color: var(--text-primary);
   }
   .modal-close {
       background: none;
       border: none;
       color: var(--text-secondary);
       font-size: 1.25rem;
       cursor: pointer;
       padding: 0.25rem 0.5rem;
   }
   .modal-close:hover {
       color: var(--text-primary);
   }
   .modal-body {
       padding: 1.5rem;
   }
   ```

   The CSS variables (`--bg-secondary`, `--border-color`, `--radius-md`, `--shadow-card`, `--text-primary`, `--text-secondary`) are all defined in the `:root` block at the top of `style.css` (lines 9-32).

**What NOT to do:**
- Do **not** import `Modal` from any other file in this task. It must compile as dead code.
- Do **not** wrap the body in a `<Show>` or `move ||` reactive closure — that defeats the always-rendered design.
- Do **not** use `Children` instead of `ChildrenFn` — `Children` is `FnOnce` and won't compose with `on:click` handlers cleanly under the always-rendered design.

**Steps:**
- [ ] Modify `crates/koji-web/Cargo.toml` to add `"KeyboardEvent"` to the `web-sys` features list.
- [ ] Run `cargo build --workspace` to verify the Cargo.toml edit alone compiles cleanly (no source changes yet, so this is a sanity check).
  - Did it succeed? Must pass before continuing.
- [ ] Create `crates/koji-web/src/components/modal.rs` with the contents specified above.
- [ ] Add `pub mod modal;` to `crates/koji-web/src/components/mod.rs`.
- [ ] Append the modal CSS block to `crates/koji-web/style.css`.
- [ ] Run `cargo build --workspace`.
  - Did it succeed? If not, the most likely failures are: missing `KeyboardEvent` import, `Closure` API mismatch, or `Callback::run` signature drift. Fix and re-run before continuing.
- [ ] Run `make build-frontend-dev` to verify the WASM build also succeeds (the workspace build above only builds koji-web for the host target).
  - Did it succeed? Common WASM-only failures: web-sys feature still missing despite the Cargo.toml edit (re-check the file), or `wasm32-unknown-unknown` target not installed (the Makefile target installs it via `rustup target add` automatically). Fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`.
  - Did it pass? Common issues: unused import, missing doc comment on a public item. Fix and re-run.
- [ ] Run `cargo fmt --all`.
- [ ] Run `cargo test --workspace`.
  - Did all existing tests still pass? They should — this task adds dead code only. If anything regressed, stop and investigate before continuing.
- [ ] Commit with message: `feat(web): add Modal shell component`

**Acceptance criteria:**
- [ ] `crates/koji-web/src/components/modal.rs` exists and exports `Modal`.
- [ ] `cargo build --workspace` passes.
- [ ] `cargo clippy --workspace -- -D warnings` passes.
- [ ] `cargo test --workspace` shows the same pass/fail counts as before this task.
- [ ] `KeyboardEvent` appears in the `web-sys` features list in `crates/koji-web/Cargo.toml`.
- [ ] `style.css` ends with the modal CSS block.
- [ ] No other file in the repo references `Modal` yet.

---

## Task 2: Extract `PullQuantWizard` and rewire `pages/pull.rs`

**Context:**
Today, `crates/koji-web/src/pages/pull.rs` contains a self-contained 6-step wizard (~600 LOC) that the user accesses at `/pull` to pull GGUF models from HuggingFace. We want to reuse this wizard from the model edit page too. This task hoists the entire state machine into a new reusable component `crates/koji-web/src/components/pull_quant_wizard.rs`, parameterized by three optional props (`initial_repo`, `is_open`, `on_complete`/`on_close`), and shrinks `pages/pull.rs` to a thin wrapper that mounts the new component with no special props (which makes it behave identically to today).

The wizard's existing state machine (`RepoInput → LoadingQuants → SelectQuants → SetContext → Downloading → Done`) is moved verbatim. The new additions on top of the verbatim move are:

1. A `CompletedQuant` public struct (spec §5.2) emitted via `on_complete`.
2. A local copy of `infer_quant_from_filename` (spec §5.5) — duplicated from `crates/koji-core/src/models/pull.rs:283` because `koji-core` is `ssr`-only and can't compile to WASM.
3. A `did_complete: RwSignal<bool>` flag.
4. A **reset Effect** that runs on `(closed → open)` transitions when `is_open` is `Some`. **It tracks ONLY `is_open`** — `wizard_step` and `initial_repo` are read via `get_untracked` to prevent a race with the on_complete Effect on the `Done` transition (spec §5.4 #1, rev 3 critical fix).
5. An **on_complete Effect** that watches `wizard_step`, fires the callback exactly once when step transitions to `Done` and `did_complete == false`, then sets `did_complete = true`.
6. Conditional UI elements gated on whether props are set:
   - Step indicator hides "1. Repo" pill when `initial_repo` is non-empty.
   - "Back" button on `SelectQuants` is hidden when `initial_repo` is non-empty.
   - "Cancel" / "Hide" / "Close" buttons appear on appropriate steps when `on_close` is set (spec §5.4 #4).
   - `Done` step renders a "Close" button when `on_complete.is_some()`, otherwise renders the existing "View Models →" link.
7. The wizard does **not** render `<div class="form-card card">` chrome anymore — that's now the host's responsibility (spec §5.4 #7), so `pages/pull.rs` retains the wrapper but the model editor's modal does not need to add one.

The verification gate for this task is that visiting `/pull` in a running koji instance still works end-to-end, identically to before — same step indicator, same buttons, same downloads, same "View Models →" link. This is **manual smoke test 1** in spec §9.

**Files:**
- Create: `crates/koji-web/src/components/pull_quant_wizard.rs`
- Modify: `crates/koji-web/src/components/mod.rs` (add `pub mod pull_quant_wizard;`)
- Modify: `crates/koji-web/src/pages/pull.rs` (shrink to thin wrapper)

**What to implement:**

### 2a. Create `crates/koji-web/src/components/pull_quant_wizard.rs`

Move the following items from `pages/pull.rs` verbatim into the new file. Line
numbers below are approximate — grep by name to be safe (`grep -n 'struct
QuantEntry\|struct JobProgress\|...' crates/koji-web/src/pages/pull.rs`):

- **Types** (around lines 10, 17, 28, 36, 47, 71, 77):
  - `struct QuantEntry { filename, quant, size_bytes }`
  - `struct JobProgress { job_id, filename, status, bytes_downloaded, total_bytes, error }`
  - `struct PullJobEntry { job_id, filename, status }`
  - `struct SsePayload { job_id, status, bytes_downloaded, total_bytes, error }`
  - `enum WizardStep { RepoInput, LoadingQuants, SelectQuants, SetContext, Downloading, Done }`
  - `struct PullRequest { repo_id, quants: Vec<QuantRequest> }`
  - `struct QuantRequest { filename, quant, context_length }`
- **Free functions** (around lines 58, 85):
  - `fn format_bytes(bytes: i64) -> String`
  - `fn step_class(current: &WizardStep, target: &WizardStep, target_idx: usize) -> &'static str`
- **Imports**: all of the existing `use` block at the top of `pages/pull.rs` (gloo_net, futures_util, leptos, serde, wasm_bindgen_futures, std::collections, etc.)

These types, functions, and imports become **private** to the new module (no `pub`). The struct fields are already `pub(crate)`-implicit because they don't escape the module — leave them as in the original.

**Add the following new items** to the new file:

```rust
/// A quant that was successfully downloaded by the wizard. Emitted via the
/// `on_complete` callback so the host can merge new quants into its own state.
#[derive(Clone, Debug)]
pub struct CompletedQuant {
    /// Exactly the repo_id the wizard used (no -GGUF auto-resolution happens
    /// today; see the spec §12 for the pre-existing gap).
    pub repo_id: String,
    /// e.g. "Qwen3-8B-Q4_K_M.gguf"
    pub filename: String,
    /// e.g. "Q4_K_M". Built via the same three-step fallback as the backend's
    /// `_setup_model_after_pull_with_config`: the HF listing's quant label,
    /// else `infer_quant_from_filename`, else None (host falls back to the
    /// trimmed filename).
    pub quant: Option<String>,
    /// Final downloaded byte count. Sourced from the SSE done payload's
    /// `bytes_downloaded` field, which equals the actual on-disk file size
    /// because `download_chunked` writes bytes 1:1.
    ///
    /// Always `Some` today (`bytes_downloaded` is `u64`, never absent for a
    /// completed job). Wrapped in `Option` for forward-compat: a future
    /// backend revision that reports completion without a final byte count
    /// can set this to `None` and the editor's merge logic
    /// (`if cq.size_bytes.is_some() { ... }`) handles it correctly without
    /// clobbering an existing value.
    pub size_bytes: Option<u64>,
    /// Context length the user picked in step 4.
    pub context_length: u32,
}

/// Local copy of `infer_quant_from_filename` from
/// `crates/koji-core/src/models/pull.rs` (around line 283). **MUST stay in
/// sync** with that function. Duplicated here because `koji-core` is only
/// available under the `ssr` feature and pulls in tokio/sqlite/reqwest, which
/// can't compile to WASM. If `koji-core` is later split into a WASM-compatible
/// utility crate, replace this with a direct import.
fn infer_quant_from_filename(filename: &str) -> Option<String> {
    let stem = filename.strip_suffix(".gguf")?;

    // Ordered longest-first so "Q4_K_M" matches before "Q4_K".
    let quant_patterns = [
        "IQ2_XXS", "IQ3_XXS", "IQ1_S", "IQ1_M", "IQ2_XS", "IQ2_S", "IQ2_M",
        "IQ3_XS", "IQ3_S", "IQ3_M", "IQ4_XS", "IQ4_NL", "Q2_K_S", "Q3_K_S",
        "Q3_K_M", "Q3_K_L", "Q4_K_S", "Q4_K_M", "Q4_K_L", "Q5_K_S", "Q5_K_M",
        "Q5_K_L", "Q2_K_XL", "Q3_K_XL", "Q4_K_XL", "Q5_K_XL", "Q6_K_XL",
        "Q8_K_XL", "Q2_K", "Q3_K", "Q4_K", "Q5_K", "Q6_K", "Q4_0", "Q4_1",
        "Q5_0", "Q5_1", "Q6_0", "Q8_0", "Q8_1", "F16", "F32", "BF16",
    ];

    let stem_upper = stem.to_uppercase();
    for pattern in &quant_patterns {
        if stem_upper.ends_with(pattern)
            || stem_upper.contains(&format!("-{}", pattern))
            || stem_upper.contains(&format!(".{}", pattern))
            || stem_upper.contains(&format!("_{}", pattern))
        {
            return Some(pattern.to_string());
        }
    }
    None
}
```

**The component signature:**

```rust
#[component]
pub fn PullQuantWizard(
    /// Pre-set HF repo ID. If non-empty AND `is_open` transitions to true,
    /// the wizard skips step 1 and immediately fetches quants. If empty,
    /// the wizard starts at the repo-input step.
    #[prop(into)] initial_repo: Signal<String>,

    /// Whether the wizard is currently visible. Convention: `None` means
    /// "hosted directly on a page, always visible, never auto-reset" — the
    /// reset Effect is not registered. `Some(signal)` enables the modal
    /// lifecycle where (closed → open) transitions drive reset/refetch.
    #[prop(into, optional)] is_open: Option<Signal<bool>>,

    /// Called once after all downloads in the current session reach a terminal
    /// state. Receives the list of quants that completed successfully (failed
    /// jobs are filtered out). Fires exactly once per session, guarded by
    /// `did_complete`.
    #[prop(optional)] on_complete: Option<Callback<Vec<CompletedQuant>>>,

    /// Called when the user dismisses via in-step Cancel/Hide/Close button.
    /// Wizard never hides itself — host decides what happens.
    #[prop(optional)] on_close: Option<Callback<()>>,
) -> impl IntoView
```

**Component body — adapt the existing `Pull` function body from `pages/pull.rs`:**

1. **Signals** — keep all existing signals from `Pull`:
   - `wizard_step`, `repo_id`, `available_quants`, `selected_filenames`, `context_lengths`, `download_jobs`, `error_msg`.
   - **Add new:** `let did_complete = RwSignal::new(false);`

2. **Reset Effect** (only registered if `is_open.is_some()`):

   ```rust
   if let Some(is_open_sig) = is_open {
       Effect::new(move |_| {
           // Subscribe ONLY to is_open. Reading other signals tracked here
           // would race with the on_complete Effect on the Done transition.
           let open = is_open_sig.get();
           if !open {
               return;
           }
           let repo = initial_repo.get_untracked();
           if repo.trim().is_empty() {
               return;
           }
           let step = wizard_step.get_untracked();
           if !matches!(step, WizardStep::RepoInput | WizardStep::Done) {
               // Mid-flow session — preserve it across close/reopen.
               return;
           }
           // Reset session state.
           selected_filenames.set(std::collections::HashSet::new());
           context_lengths.set(std::collections::HashMap::new());
           download_jobs.set(Vec::new());
           error_msg.set(None);
           did_complete.set(false);
           repo_id.set(repo.clone());
           wizard_step.set(WizardStep::LoadingQuants);

           // Spawn the same fetch the Search button does today.
           wasm_bindgen_futures::spawn_local(async move {
               let url = format!("/koji/v1/hf/{}", repo);
               match gloo_net::http::Request::get(&url).send().await {
                   Ok(resp) => match resp.json::<Vec<QuantEntry>>().await {
                       Ok(quants) => {
                           if quants.is_empty() {
                               error_msg.set(Some(
                                   "No GGUF files found for this repo. Check the repo ID and try again.".to_string(),
                               ));
                               wizard_step.set(WizardStep::RepoInput);
                           } else {
                               available_quants.set(quants);
                               wizard_step.set(WizardStep::SelectQuants);
                           }
                       }
                       Err(e) => {
                           error_msg.set(Some(format!("Failed to parse response: {e}")));
                           wizard_step.set(WizardStep::RepoInput);
                       }
                   },
                   Err(e) => {
                       error_msg.set(Some(format!("Request failed: {e}")));
                       wizard_step.set(WizardStep::RepoInput);
                   }
               }
           });
       });
   }
   ```

   This is the **single most subtle piece of the task**. It MUST track only `is_open` (one tracked `.get()` call), reading everything else with `get_untracked()`. Do not refactor this Effect to "be more reactive" or "read `wizard_step` so it auto-runs on transitions" — that reintroduces the race.

3. **on_complete Effect** (only registered if `on_complete.is_some()`):

   ```rust
   if let Some(cb) = on_complete {
       Effect::new(move |_| {
           let step = wizard_step.get();
           if step != WizardStep::Done {
               return;
           }
           if did_complete.get_untracked() {
               return;
           }
           did_complete.set(true);

           let jobs = download_jobs.get_untracked();
           let quants_listing = available_quants.get_untracked();
           let ctx_map = context_lengths.get_untracked();
           let repo = repo_id.get_untracked();

           let completed: Vec<CompletedQuant> = jobs
               .into_iter()
               .filter(|j| j.status == "completed")
               .map(|j| {
                   let entry = quants_listing.iter().find(|q| q.filename == j.filename);
                   let quant = entry
                       .and_then(|e| e.quant.clone())
                       .or_else(|| infer_quant_from_filename(&j.filename));
                   let context_length = ctx_map.get(&j.filename).copied().unwrap_or(32768);
                   CompletedQuant {
                       repo_id: repo.clone(),
                       filename: j.filename.clone(),
                       quant,
                       // Always Some today — `bytes_downloaded` is u64, never None.
                       // Wrapped in Option for forward-compat (e.g. if a future
                       // backend reports completion without a final byte count).
                       size_bytes: Some(j.bytes_downloaded),
                       context_length,
                   }
               })
               .collect();

           cb.run(completed);
       });
   }
   ```

4. **Skip-step-1 mount logic** — when there's no `is_open` signal but `initial_repo` is already populated (which is currently never the case, since `Pull()` passes empty), the existing reset logic won't fire. That's fine: the hosting page is responsible for triggering whatever flow it wants. For the model editor case, `is_open` is always passed, so the reset Effect handles initial fetch.

   **Important:** there is no code path in this task that auto-fetches without `is_open` being set. The current `/pull` page is unaffected because it doesn't pass `initial_repo` — the user types it manually and clicks Search.

5. **Render block** — copy the existing match-on-`wizard_step` view block from `pages/pull.rs` (the entire `view! {}` body), then make these modifications:

   - **Remove the outer `<div class="form-card card">` wrapper** — the host page now provides it. The wizard's top-level view is the `wizard-steps` indicator + the `match` block, no card wrapper.
   - **In the `wizard-steps` indicator**, gate the "1. Repo" pill on `initial_repo` being empty. The outer `move ||` closure is already reactive on both `wizard_step` and `initial_repo`, so a plain `then(||)` is sufficient — no `<Show>` wrapper needed:
     ```rust
     <div class="wizard-steps mb-3">
         {move || {
             let step = wizard_step.get();
             let show_repo_step = initial_repo.get().trim().is_empty();
             view! {
                 {show_repo_step.then(|| view! {
                     <div class=step_class(&step, &WizardStep::RepoInput, 0)>"1. Repo"</div>
                 })}
                 // ... remaining 5 step pills, unchanged ...
             }
         }}
     </div>
     ```
     Notes:
     - The indices `0..5` for `step_class` reflect *position in the order array*, not display position. The `step_class` function uses a fixed order array so the existing arguments stay correct even when the first pill is hidden.
     - In today's `pull.rs`, the indicator block already wraps the whole thing in `{move || { ... view! { ... } }}` (lines ~123-135). The 5 trailing pills use `step_class(&step, ...)` reading the local `step` variable, **not** their own per-pill `move ||` closures. So a verbatim move works without further surgery — the only change is wrapping the first pill in `show_repo_step.then(...)` and adding the `let show_repo_step = ...;` line.
   - **In the `SelectQuants` step**, gate the Back button on `initial_repo` being empty. The current code has:
     ```rust
     <button class="btn btn-secondary" on:click=move |_| {
         wizard_step.set(WizardStep::RepoInput);
     }>"Back"</button>
     ```
     Wrap this in `<Show when=move || initial_repo.get().trim().is_empty()>`.
   - **Add Cancel buttons** to `RepoInput`, `SelectQuants`, and `SetContext` steps when `on_close.is_some()`. Place each as the leftmost button in the existing form-actions row:
     ```rust
     {on_close.map(|cb| view! {
         <button type="button" class="btn btn-secondary" on:click=move |_| cb.run(())>
             "Cancel"
         </button>
     })}
     ```
     Note: `Callback` is `Copy`, so capturing it inside a `move` closure is fine without cloning.
   - **Add a Hide button** to the `Downloading` step (replaces nothing — there's currently no button there) when `on_close.is_some()`:
     ```rust
     {on_close.map(|cb| view! {
         <div class="form-actions mt-3">
             <button type="button" class="btn btn-secondary" on:click=move |_| cb.run(())>
                 "Hide"
             </button>
         </div>
     })}
     ```
   - **In the `Done` step**, replace the existing
     ```rust
     <div class="form-actions mt-3">
         <a href="/models">
             <button class="btn btn-primary">"View Models →"</button>
         </a>
     </div>
     ```
     with a conditional:
     ```rust
     <div class="form-actions mt-3">
         {match on_close {
             Some(cb) => view! {
                 <button type="button" class="btn btn-primary" on:click=move |_| cb.run(())>
                     "Close"
                 </button>
             }.into_any(),
             None => view! {
                 <a href="/models">
                     <button class="btn btn-primary">"View Models →"</button>
                 </a>
             }.into_any(),
         }}
     </div>
     ```

     **Spec deviation note (intentional).** Spec §5.4 #6 originally said
     "branches on `on_complete.is_some()`". This plan branches on
     `on_close.is_some()` instead, because the Close button itself calls
     `on_close` — it would be nonsense to render a Close button when
     `on_close` is `None`. Branching on `on_close` is the correct invariant.
     Both branches give identical behaviour for the two real call sites in
     this PR: `/pull` passes neither callback (→ "View Models→"), and the
     model editor passes both (→ "Close"). The spec was updated to match in
     rev 4 alongside this plan.

### 2b. Update `crates/koji-web/src/components/mod.rs`

Add `pub mod pull_quant_wizard;` alongside the existing module declarations.

### 2c. Rewrite `crates/koji-web/src/pages/pull.rs`

Delete the entire current contents and replace with:

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

That's the entire file. No imports for `gloo_net`, `EventSource`, etc. — those moved to the wizard module. No `is_open`, `on_complete`, or `on_close` props are passed, so the wizard renders identically to today.

**What NOT to do:**
- Do **not** import `Modal` here. Task 3 is the first place Modal is mounted.
- Do **not** make any change to `crates/koji-web/src/pages/model_editor.rs`. That's task 3.
- Do **not** modify `koji-core` to expose `infer_quant_from_filename` — duplicating it locally is the agreed approach (spec §5.5).
- Do **not** "improve" the reset Effect by tracking more signals reactively. The single tracked read of `is_open` is the entire correctness story.
- Do **not** add `Send + Sync` bounds explicitly anywhere — Leptos's `Callback<T>` already requires them and the captured types in this plan all satisfy them.
- Do **not** delete or reorganize the existing wizard logic for clarity. Move it verbatim. Refactoring is a future task.

**Steps:**
- [ ] Create `crates/koji-web/src/components/pull_quant_wizard.rs` with all hoisted types/functions/imports plus the new `CompletedQuant`, `infer_quant_from_filename`, `did_complete` signal, reset Effect, on_complete Effect, and gated UI elements.
- [ ] Add `pub mod pull_quant_wizard;` to `crates/koji-web/src/components/mod.rs`.
- [ ] Replace `crates/koji-web/src/pages/pull.rs` contents with the thin wrapper above.
- [ ] Run `cargo build --workspace`.
  - Did it succeed? Most likely failures: missed imports, `Show` not in scope (it's `leptos::prelude::*`), `Effect::new` signature mismatch, `Signal::derive` taking a closure that needs to be `'static`, `Callback::run` arity. Fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`.
  - Common issues: dead code warnings on the old `Pull` page's helper imports that you forgot to delete; unused `format!` arguments. Fix and re-run.
- [ ] Run `cargo fmt --all`.
- [ ] Run `cargo test --workspace`.
  - Did all existing tests still pass? If the koji-web crate has any non-WASM unit tests under `tests/` that still reference `Pull` internals, they'll need adjustment. (At spec-write time, only `tests/server_test.rs` exists, gated on `ssr` feature, and shouldn't touch wizard internals — but verify.)
- [ ] **Manual smoke test 1 — `/pull` regression** (spec §9 test 1). This is the verification gate. Steps:
  1. Build the WASM frontend: `make build-frontend-dev` (must succeed before the new code is visible in the browser).
  2. Start a local koji instance: `cargo run -p koji-cli -- serve` (binds to `127.0.0.1:11434` by default — see `crates/koji-cli/src/cli.rs:114-122`).
  3. Open <http://127.0.0.1:11434/pull> in a browser.
  4. The "1. Repo" pill is visible in the wizard step indicator (because `initial_repo` is empty).
  5. Enter a real HF repo ID (use `bartowski/Qwen3-0.6B-GGUF` — small download, well-known to work) and click "Search".
  6. The wizard transitions to `LoadingQuants`, then `SelectQuants` with a populated table.
  7. Select one small quant (Q4_K_M or smaller) and click "Next →".
  8. On `SetContext`, leave the default and click "Start Download".
  9. The wizard transitions to `Downloading` with a live progress bar that updates from SSE.
  10. After completion, the wizard reaches `Done` and shows the "View Models →" link (NOT "Close" — `on_close` is not set on `/pull`).
  11. Click "View Models →" — it navigates to `/models`.
  - If any step deviates from the above, stop and investigate. The wizard must behave identically to before this task. Stop the server with Ctrl-C when done.
- [ ] If smoke test 1 passes, commit with message: `refactor(web): extract PullQuantWizard component from /pull page`

**Acceptance criteria:**
- [ ] `crates/koji-web/src/components/pull_quant_wizard.rs` exists and exports `PullQuantWizard` and `CompletedQuant`.
- [ ] `crates/koji-web/src/pages/pull.rs` is ≤ 25 lines and imports `PullQuantWizard`.
- [ ] `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` all pass.
- [ ] Manual smoke test 1 (`/pull` regression) passes — the page behaves identically to before, end-to-end download verified.
- [ ] The reset Effect's body has exactly one `is_open_sig.get()` call and uses `get_untracked()` for `initial_repo`, `wizard_step`, etc.
- [ ] The on_complete Effect uses `did_complete` as a one-shot guard and sets it to true before invoking the callback.

---

## Task 3: Wire the modal+wizard into the model editor

**Context:**
This is the user-facing payoff. The model edit page (`crates/koji-web/src/pages/model_editor.rs`) currently has a "+ Add Quant" button that just appends an empty row to the local `quants` signal. We replace it with a "+ Pull Quant" button that opens the wizard inside a modal, pre-filled with the current model's HF repo ID (`form_model`). When downloads complete, an `on_complete` callback merges the new quants into the local `quants` signal field-by-field, preserving any other unsaved form edits.

The button is disabled when `form_model` is empty, with a tooltip carried by a wrapping `<span>` (Firefox doesn't show `title` on disabled form controls). Spec §8 fully describes this section.

The modal is mounted with `pull_modal_open: RwSignal<bool>`. Because the Modal component (from task 1) renders its children unconditionally and toggles visibility via CSS, the wizard's signals + SSE futures are preserved across close/reopen. This means:

- The user can close the modal mid-download → downloads keep running → `on_complete` fires whenever downloads finish → quants merge into the editor's table even if the modal is hidden.
- On reopen, the wizard's reset Effect (from task 2) checks `wizard_step`: if it's `Done` or `RepoInput`, it resets and refetches; if it's mid-flow (`LoadingQuants`/`SelectQuants`/`SetContext`/`Downloading`), it preserves the session.

This task includes manual smoke tests 2-8 from the spec.

**Files:**
- Modify: `crates/koji-web/src/pages/model_editor.rs`

**What to implement:**

### 3a. Imports

At the top of `model_editor.rs`, add:

```rust
use crate::components::modal::Modal;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
```

(Confirm the existing imports use `crate::components::nav::Nav` or similar pattern — match the project's convention. If imports use `use leptos::prelude::*;` already, the additions are independent.)

### 3b. New signal — replace `add_quant_counter`

Locate the existing line near line 281:

```rust
let add_quant_counter = RwSignal::new(0);
```

**Delete this line** and replace with:

```rust
let pull_modal_open = RwSignal::new(false);
```

Then locate the only use site of `add_quant_counter` (in the "+ Add Quant" button's `on:click` handler around line 1173-1174):

```rust
let counter = add_quant_counter.get() + 1;
add_quant_counter.set(counter);
let unique_name = format!("quant-{}", counter);
quants.update(|rows| rows.push((unique_name, QuantInfo::default())));
```

This whole block goes away as part of the button replacement in 3c.

### 3c. Replace the "+ Add Quant" button

Locate the existing button in `model_editor.rs` around lines 1170-1180:

```rust
<div class="mt-1">
    <button
        type="button"
        class="btn btn-secondary btn-sm"
        on:click=move |_| {
            let counter = add_quant_counter.get() + 1;
            add_quant_counter.set(counter);
            let unique_name = format!("quant-{}", counter);
            quants.update(|rows| rows.push((unique_name, QuantInfo::default())));
        }
    >"+ Add Quant"</button>
</div>
```

Replace with:

```rust
<div class="mt-1">
    <span title=move || if form_model.get().trim().is_empty() {
        "Enter the HuggingFace repo above before pulling quants".to_string()
    } else {
        "Pull a new quant from HuggingFace".to_string()
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

The wrapping `<span>` is the Firefox workaround for `title` on disabled buttons. The closure returning `String` (rather than `&str`) is required because the branches return owned strings — Leptos will accept either, but consistent ownership avoids lifetime headaches.

### 3d. Mount the modal + wizard

At the bottom of the model editor's main view, mount the modal as a **sibling
of the form's outer `<div>`** — i.e. as a second top-level node inside the
success-branch `view! { }` block, after the closing `</div>` that follows
`</form>` but before the closing `}.into_any()`. The relevant structure today
(verified at `crates/koji-web/src/pages/model_editor.rs:1208-1224`):

```rust
                            // ... model_status alert is INSIDE the form ...
                        </form>          // line 1221
                    </div>               // line 1222 — closes the outer form-card div
                                         // ← add the <Modal>...</Modal> here, as a sibling
                }.into_any()             // line 1223
            }}
        </Suspense>
```

Add:

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
            // Visibility for the silent-failure caveat in spec §8.7: if all
            // quants in this session failed, log to console so the user has
            // *some* trace after the modal auto-closes.
            if completed.is_empty() {
                web_sys::console::warn_1(
                    &"All pulled quants failed; nothing merged into the editor.".into(),
                );
            }
            quants.update(|rows| {
                for cq in completed {
                    let key = cq.quant.clone()
                        .unwrap_or_else(|| {
                            cq.filename.trim_end_matches(".gguf").to_string()
                        });
                    if let Some(pos) = rows.iter().position(|(k, _)| k == &key) {
                        // Re-pull: overwrite filename and context_length
                        // (the wizard's values reflect the user's latest intent).
                        // Only overwrite size_bytes when we have a value —
                        // never clobber a known size with None.
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

**Why inside the Suspense success branch:** the `<Modal>` must live inside the
same view branch that renders the form, so the trigger button and the modal
mount together when the model finishes loading. Placing the Modal outside the
Suspense would still compile (`form_model` and `quants` are declared at the
top of `Editor` before the Suspense, see line 280-281), but the modal would
be mounted on first render before the trigger button exists — unnecessary work
and a confusing intermediate state.

### 3e. Verify nothing else references `add_quant_counter`

Run `grep -n add_quant_counter crates/koji-web/src/pages/model_editor.rs` after the edits — should return zero matches. Same for the literal string `"+ Add Quant"`.

**What NOT to do:**
- Do **not** call `quants.set(...)` in `on_complete` — use `quants.update(...)`. The set form would discard the existing rows.
- Do **not** re-fetch the model from `/api/models/:id` in `on_complete`. Decision 4 in the spec is "client-side merge, preserve unsaved form edits" — re-fetching would discard those edits.
- Do **not** add a busy-state guard preventing modal close during downloads. Decision 6 explicitly chose option A: closing mid-download is allowed and downloads continue in the background.
- Do **not** modify the Save button's serialization logic. The existing path that converts `quants: Vec<(String, QuantInfo)>` into `BTreeMap` on save (around line 547) handles new rows correctly.
- Do **not** modify `crates/koji-web/src/pages/pull.rs` further. Task 2 already handled it.
- Do **not** add a toast / log for mid-download close. Spec §8.7 documents it as a known caveat for v1.

**Steps:**
- [ ] Add the two `use` lines for `Modal` and `PullQuantWizard`/`CompletedQuant`.
- [ ] Replace the `add_quant_counter` signal with `pull_modal_open`.
- [ ] Replace the "+ Add Quant" button block with the "+ Pull Quant" button (wrapped in `<span title=...>`, with `prop:disabled` gated on `form_model`).
- [ ] Add the `<Modal>` + `<PullQuantWizard>` mount at the bottom of the success view branch.
- [ ] Run `grep -n add_quant_counter crates/koji-web/src/pages/model_editor.rs` — should return zero matches.
- [ ] Run `cargo build --workspace`.
  - Did it succeed? Likely failures: closure `'static` errors (move things into the closure), `Callback::new` signature, `Signal::derive` lifetime, missing import. Fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`.
- [ ] Run `cargo fmt --all`.
- [ ] Run `cargo test --workspace`.
  - All existing tests should still pass.
- [ ] **Manual smoke test 2 — disabled state** (spec §9 test 2):
  1. Start the koji web UI (`make build-frontend-dev && cargo run -p koji-cli -- serve`) and open <http://127.0.0.1:11434/models/new/edit> in a browser. (The route is `/models/:id/edit` per `crates/koji-web/src/lib.rs:24`, with `id == "new"` as a special case handled in `model_editor::fetch_model`.)
  2. The "+ Pull Quant" button is visible and **disabled**.
  3. Hover over the button — a tooltip appears reading "Enter the HuggingFace repo above before pulling quants".
  4. Type a value into the model field (e.g. `bartowski/Qwen3-0.6B-GGUF`). The button enables.
  5. Hover again — tooltip now reads "Pull a new quant from HuggingFace".
- [ ] **Manual smoke test 3 — happy path** (spec §9 test 3):
  1. Navigate to an existing model with a populated `model` field (use a `-GGUF` repo).
  2. Click "+ Pull Quant" — the modal opens, dim background visible.
  3. The wizard immediately enters `LoadingQuants` (no "1. Repo" pill in the indicator).
  4. The list of quants appears. Select one small quant.
  5. Click "Next →", set context, click "Start Download". Progress bar updates.
  6. On `Done`, click the **"Close"** button (NOT "View Models →").
  7. The modal closes. The new quant row is visible in the editor's quants table with the correct `file`, `size_bytes`, and `context_length`.
  8. Click "Save Model" — no errors. Reload the page and confirm the row is still there with the same values.
- [ ] **Manual smoke test 4 — re-pull (upsert)** (spec §9 test 4):
  1. Same model as test 3. Open the modal again.
  2. Select the **same** quant as before. Set a **different** context length.
  3. Complete the download.
  4. After the modal closes, the existing row in the quants table is updated in place (NOT duplicated). `context_length` reflects the new value.
- [ ] **Manual smoke test 5 — cancel paths** (spec §9 test 5):
  1. Open modal, click X in the header → closes.
  2. Open modal, press Escape → closes.
  3. Open modal, click on the dim backdrop area outside the modal → closes.
  4. Open modal, click the in-step "Cancel" button on the SelectQuants step → closes.
  5. After each: no quant rows added.
- [ ] **Manual smoke test 6 — preserves unsaved edits** (spec §9 test 6):
  1. Open an existing model. Type a NEW value into the "Display name" field. Do NOT save.
  2. Click "+ Pull Quant" and complete a pull.
  3. After modal closes, the display-name edit is still in the field.
- [ ] **Manual smoke test 7 — mid-download close, reopen** (spec §9 test 7):
  1. Open modal, start a download of a reasonably large quant.
  2. Before completion, close the modal via X.
  3. Reopen the modal. It must show the still-running `Downloading` step with the in-progress bar continuing — NOT a fresh `LoadingQuants` reset.
  4. Wait for completion. The new row appears in the quants table.
- [ ] **Manual smoke test 8 — modal-closed completion** (spec §9 test 8):
  1. Open modal, start a download.
  2. Close the modal mid-download.
  3. Wait for the download to complete in the background (without reopening).
  4. The new row appears in the editor's quants table without the user needing to interact with the modal again.
- [ ] If all smoke tests pass, commit with message: `feat(web): pull quants from HuggingFace via modal on model edit page`

**Acceptance criteria:**
- [ ] `add_quant_counter` no longer exists in `model_editor.rs`.
- [ ] The "+ Pull Quant" button is `prop:disabled` when `form_model.get().trim().is_empty()`.
- [ ] The button is wrapped in a `<span title=...>` for the Firefox tooltip workaround.
- [ ] The Modal mounts the PullQuantWizard with all four props: `initial_repo`, `is_open`, `on_complete`, `on_close`.
- [ ] `on_complete` does field-by-field upsert: overwrites `file` and `context_length` always, overwrites `size_bytes` only when `Some`.
- [ ] `on_complete` logs a `console.warn` when `completed.is_empty()`.
- [ ] All 7 smoke tests (2-8) pass.
- [ ] `cargo build --workspace`, `make build-frontend-dev`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all`, `cargo test --workspace` all pass.

---

## Final verification (after all 3 tasks)

Before pushing the branch / opening a PR:

- [ ] On `feature/pull-quant-from-model-editor`, run the full check suite. The project's canonical gate is `make check` (which is `make fmt clippy test`):
  ```bash
  make fmt
  make clippy             # runs both workspace clippy AND koji-web --features ssr clippy
  make test               # rebuilds the WASM frontend AND runs both workspace + ssr test passes
  cargo build --workspace # final sanity build
  ```
  All must pass. If `make` is unavailable, the equivalent is:
  ```bash
  cargo fmt --all
  cargo clippy --workspace -- -D warnings
  cargo clippy --package koji-web --features ssr -- -D warnings
  make build-frontend-dev   # or: cd crates/koji-web && trunk build
  cargo test --workspace
  cargo test --package koji-web --features ssr
  cargo build --workspace
  ```
- [ ] `git log --oneline feature/pull-quant-from-model-editor` should show three feature commits (one per task) plus the spec commits from the brainstorming phase.
- [ ] `git diff main..feature/pull-quant-from-model-editor --stat` should show changes only in:
  - `crates/koji-web/src/components/mod.rs`
  - `crates/koji-web/src/components/modal.rs` (new)
  - `crates/koji-web/src/components/pull_quant_wizard.rs` (new)
  - `crates/koji-web/src/pages/pull.rs` (massively shrunk)
  - `crates/koji-web/src/pages/model_editor.rs`
  - `crates/koji-web/style.css`
  - `crates/koji-web/Cargo.toml`
  - `docs/plans/2026-04-07-pull-quant-from-model-editor-spec.md` (new, from brainstorm)
  - `docs/plans/2026-04-07-pull-quant-from-model-editor-plan.md` (new, this file)

  No backend files (`crates/koji-core`, `crates/koji-cli`) should appear. If they do, something is wrong — backend changes were explicitly out of scope.
- [ ] Push branch, open PR to `develop`.
