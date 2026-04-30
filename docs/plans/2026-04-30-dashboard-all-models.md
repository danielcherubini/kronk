# Dashboard: Show All Models + Pull Model + Check All for Updates

**Goal:** Extend the Tama dashboard to display all models (active + inactive), add "Pull Model" and "Check all for updates" buttons, and hide the Models page from the sidebar while keeping it accessible via direct URL.

**Architecture:** The dashboard (`dashboard.rs`) currently shows only active models (ready/loading/unloading) via SSE metrics. We add an `inactive_models()` helper to compute the complement, render a second section below, and wire two new header actions (Pull Model modal + Check all for updates). The Models page route stays registered but is hidden from navigation.

**Tech Stack:** Rust, Leptos CSR (WASM), `gloo_net::http`, `serde`, `serde_json`

---

## Task 1: Extract `rw_signal_to_signal` to `crate::utils`

**Context:**
The `rw_signal_to_signal` helper function exists as a private function in two files (`models.rs` and `model_editor/mod.rs`) and is needed by the dashboard to wire the `PullQuantWizard` modal. Extracting it to a shared location eliminates duplication and ensures consistency across all three pages.

**Files:**
- Modify: `crates/tama-web/src/utils.rs`
- Modify: `crates/tama-web/src/pages/models.rs`
- Modify: `crates/tama-web/src/pages/model_editor/mod.rs`

**What to implement:**
1. In `crates/tama-web/src/utils.rs`, add the `rw_signal_to_signal` function at the bottom of the file (after the last function, before the `mod utils` tests if present):
```rust
/// Convert an `RwSignal<T>` to a read-only `Signal<T>` by splitting and returning the read half.
pub fn rw_signal_to_signal<T: Clone + Send + Sync + 'static>(sig: RwSignal<T>) -> Signal<T> {
    let (read, _) = sig.split();
    read.into()
}
```
2. In `crates/tama-web/src/pages/models.rs`, remove the private `rw_signal_to_signal` function and replace all calls with `crate::utils::rw_signal_to_signal`.
3. In `crates/tama-web/src/pages/model_editor/mod.rs`, remove the private `rw_signal_to_signal` function and replace all calls with `crate::utils::rw_signal_to_signal`.

**Steps:**
- [ ] Add `rw_signal_to_signal` function to `crates/tama-web/src/utils.rs` with the exact signature and doc comment above
- [ ] In `crates/tama-web/src/pages/models.rs`, find and delete the private `fn rw_signal_to_signal<T: Clone + Send + Sync + 'static>(sig: RwSignal<T>) -> Signal<T>` function (approximately line 28)
- [ ] In `crates/tama-web/src/pages/models.rs`, replace all calls to `rw_signal_to_signal(` with `crate::utils::rw_signal_to_signal(`
- [ ] In `crates/tama-web/src/pages/model_editor/mod.rs`, find and delete the private `fn rw_signal_to_signal` function
- [ ] In `crates/tama-web/src/pages/model_editor/mod.rs`, replace all calls to `rw_signal_to_signal(` with `crate::utils::rw_signal_to_signal(`
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "refactor(tama-web): extract rw_signal_to_signal to crate::utils"

**Acceptance criteria:**
- [ ] `rw_signal_to_signal` exists in `crate::utils` with public visibility
- [ ] No private copies remain in `models.rs` or `model_editor/mod.rs`
- [ ] All calls to `rw_signal_to_signal` resolve via `crate::utils::rw_signal_to_signal`
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings

---

## Task 2: Add `inactive_models()` helper and compute both lists

**Context:**
The dashboard currently computes `active = active_models(&all_models)` but has no equivalent for inactive models. Adding `inactive_models()` as a symmetric complement enables rendering the Inactive Models section. This is a pure data transformation with no UI changes — the UI section comes in Task 3.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard.rs`

**What to implement:**
1. In `crates/tama-web/src/pages/dashboard.rs`, add the `inactive_models()` function right after the existing `active_models()` function (they should be adjacent for easy comparison):
```rust
/// Returns models whose state is NOT one of the "active" states.
/// These are models that are idle, failed, or otherwise not running.
fn inactive_models(models: &[ModelStatus]) -> Vec<ModelStatus> {
    models
        .iter()
        .filter(|m| !matches!(m.state.as_str(), "ready" | "loading" | "unloading"))
        .cloned()
        .collect()
}
```
2. In the `Dashboard` component's view closure, find the existing line:
```rust
let all_models: Vec<ModelStatus> = buf.last().map(|h| h.models.clone()).unwrap_or_default();
let models = active_models(&all_models);
```
Replace it with:
```rust
let all_models: Vec<ModelStatus> = buf.last().map(|h| h.models.clone()).unwrap_or_default();
let active = active_models(&all_models);
let inactive = inactive_models(&all_models);
```
3. Update all subsequent references from `models` to `active` — specifically:
   - `format!("{} loaded", models.len())` → `format!("{} loaded", active.len())`
   - `else if models.is_empty()` → `else if active.is_empty()`
   - `let mut sorted = models;` → `let mut sorted = active;`

**Do NOT change:** The **behavior** or **structure** of the Active Models section — only rename the `models` variable to `active`. No changes to `ModelRow`, CSS, or the rendering logic.

**Steps:**
- [ ] Add `inactive_models()` function to `crates/tama-web/src/pages/dashboard.rs` right after `active_models()`
- [ ] Replace `let models = active_models(&all_models);` with `let active = active_models(&all_models);` and `let inactive = inactive_models(&all_models);`
- [ ] Replace all uses of `models` (where it refers to the active models list) with `active`
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(tama-web): add inactive_models() helper for dashboard"

**Acceptance criteria:**
- [ ] `inactive_models()` exists and correctly filters non-active states
- [ ] `active` and `inactive` variables are computed from `all_models`
- [ ] All references to the active models list use `active` (not `models`)
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings
- [ ] The existing "Active Models" section still compiles and renders correctly (no behavioral change)

---

## Task 3: Add Inactive Models UI section to dashboard

**Context:**
With the data separation in Task 2, we can now render inactive models. This task adds a second `<section>` below the Active Models section, using the existing `ModelRow` component (which already handles idle/failed states correctly — idle shows "Load"/green, failed shows "Retry"/yellow).

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard.rs`

**What to implement:**
Insert a new `<section class="dashboard-models">` immediately after the existing Active Models `</section>` tag. The new section follows the same structure as the Active Models section but uses `inactive` instead of `active`.

**CRITICAL:** The entire section must be wrapped in a conditional that checks `all_models.is_empty()`. When `all_models` is empty, skip this section entirely (the Active section already shows "No models configured yet.").

The section structure (wrapped in a conditional):
```rust
// Inactive Models section — only render when all_models is non-empty
if !all_models.is_empty() {
    view! {
        <section class="dashboard-models">
            <div class="page-header">
                <h2>"Inactive Models"</h2>
                <span class="text-muted">
                    {format!("{} inactive", inactive.len())}
                </span>
            </div>
            {
                if inactive.is_empty() {
                    view! {
                        <div class="card card--centered">
                            <p class="text-muted">"No inactive models."</p>
                        </div>
                    }.into_any()
                } else {
                    // Sort by id (stable order, matching the backend)
                    let mut sorted = inactive;
                    sorted.sort_by(|a, b| a.id.cmp(&b.id));
                    view! {
                        <div class="models-list">
                            {sorted.into_iter().map(|m| {
                                let display_name = model_display_name(&m);
                                let quant_display: String = m
                                    .quant
                                    .as_deref()
                                    .unwrap_or("\u{2014}")
                                    .into();
                                let context_display = m.context_length.map(|n| {
                                    if n >= 1024 && n % 1024 == 0 {
                                        format!("{}k", n / 1024)
                                    } else if n >= 1000 && n % 1000 == 0 {
                                        format!("{}k", n / 1000)
                                    } else {
                                        n.to_string()
                                    }
                                }).unwrap_or_else(|| "—".to_string());
                                let backend_name = format!("{}_{}", m.backend, m.id);
                                let id = m.id.clone();
                                let db_id = m.db_id;
                                let state = m.state.clone();
                                let on_load_cb = Callback::new(move |id: String| {
                                    load_action.dispatch(id);
                                });
                                let on_unload_cb = Callback::new(move |id: String| {
                                    unload_action.dispatch(id);
                                });
                                view! {
                                    <ModelRow
                                        id=id
                                        db_id=db_id
                                        display_name=display_name
                                        quant_display=quant_display
                                        context_display=context_display
                                        backend_name=backend_name
                                        state=state
                                        load_pending=load_busy
                                        unload_pending=unload_busy
                                        on_load=on_load_cb
                                        on_unload=on_unload_cb
                                    />
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    }.into_any()
                }
            }
        </section>
    }.into_any()
} else {
    view! { <div></div> }.into_any()
}
```

**Important:** The `ModelRow` component already handles idle/failed states correctly:
- Idle → "Load" button (green `btn-success`)
- Failed → "Retry" button (yellow `btn-warning`)
- Loading/unloading → disabled secondary button

No changes to `ModelRow` are needed.

**Steps:**
- [ ] Add the Inactive Models `<section>` to `crates/tama-web/src/pages/dashboard.rs` immediately after the existing Active Models `</section>`
- [ ] Use `inactive` (not `active`) for the model list
- [ ] Include the empty-state precedence logic (skip if `all_models.is_empty()`, show "No inactive models." if empty but `all_models` non-empty, render rows if `inactive` non-empty)
- [ ] Use the same `ModelRow` component with the same props as the Active Models section
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(tama-web): add Inactive Models section to dashboard"

**Acceptance criteria:**
- [ ] Inactive Models section renders below Active Models section
- [ ] Empty `all_models` → only "No models configured yet." shown (Inactive section skipped)
- [ ] Non-empty `all_models` with no inactive → "No inactive models." shown
- [ ] Non-empty inactive list → model rows rendered with correct Load/Retry buttons
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings

---

## Task 4: Add "Pull Model" button and "Check all for updates" to dashboard header

**Context:**
The models page has two features the dashboard lacks: "Pull Model" (opens a wizard modal) and "Check all for updates" (refreshes metadata for all models). These need to be added to the dashboard header.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard.rs`

**What to implement:**

#### 4a. Pull Model button

Add a `pull_modal_open: RwSignal<bool>` signal near the top of the `Dashboard` component (after existing signals like `load_busy` and `unload_busy`).

Add a `<Modal>` + `<PullQuantWizard>` block at the end of the `view!` macro (before the closing `}` of the main `view!`).

Wire the header button to open the modal.

Details:
- Signal: `let pull_modal_open = RwSignal::new(false);`
- Modal block:
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
            connect_trigger.update(|n| *n += 1);
        })
        on_close=Callback::new(move |_| pull_modal_open.set(false))
    />
</Modal>
```
- Import: `use crate::components::modal::Modal;`, `use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};`, and `use crate::utils::rw_signal_to_signal;` (extracted in Task 1)
- **Button placement (CRITICAL):** Do NOT add the button inside the existing `{move || { history.get().last().cloned().map(|_h| { ... }) }}` conditional — it would be invisible until the first SSE sample arrives. Instead, add a new `<div class="page-header__actions">` as a **direct child** of `<div class="page-header">`, outside the conditional closure. This matches the pattern used in `models.rs` and ensures buttons are always visible.

The page-header structure should become:
```rust
<div class="page-header">
    <h1>"Dashboard"</h1>
    <div class="page-header__actions">
        // Existing status badge + Restart (inside conditional, only shown after SSE data arrives)
        {move || {
            history.get().last().cloned().map(|_h| {
                let badge_class = if fetch_failed.get() { "badge badge-danger" } else { "badge badge-success" };
                let badge_text = if fetch_failed.get() { "error" } else { "ok" };
                view! {
                    <div class="flex-between gap-1">
                        <span class={badge_class}>{badge_text}</span>
                        <button class="btn btn-secondary btn-sm" on:click=move |_| { restart.dispatch(()); }>
                            "Restart"
                        </button>
                    </div>
                }
            })
        }}
        // New buttons (always visible, outside conditional)
        <button class="btn btn-secondary" on:click=move |_| pull_modal_open.set(true)>"Pull Model"</button>
        <button
            class="btn btn-secondary"
            prop:disabled=move || check_all_busy.get()
            on:click=move |_| { check_all_action.dispatch(()); }
            title="Check HuggingFace for updated metadata on every model"
        >
            {move || if check_all_busy.get() { "Checking..." } else { "Check all for updates" }}
        </button>
    </div>
</div>
```

#### 4b. Check all for updates button

Add a `check_all_busy: RwSignal<bool>` signal and `check_all_status: RwSignal<Option<(bool, String)>>` signal near the top of the component.

Add a `check_all_action: Action<(), (), LocalStorage>` that:
1. Fetches `GET /tama/v1/models` to get the list of models
2. Parses the response using a typed struct (NOT `serde_json::Value::as_str()` — that returns `None` for JSON numbers):
```rust
#[derive(Debug, Clone, serde::Deserialize)]
struct ModelsApiResponse {
    models: Vec<CheckAllModel>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct CheckAllModel {
    id: i64,
}
```
3. Iterates over each model, POSTs to `/tama/v1/models/{id}/refresh` using `post_request()` (from `crate::utils`) for CSRF safety
4. Counts successes/failures and sets `check_all_status`

**Full `check_all_action` implementation** (adapted from `models.rs` but using typed structs):
```rust
let check_all_action: Action<(), (), LocalStorage> =
    Action::new_unsync(move |_: &()| async move {
        check_all_busy.set(true);
        check_all_status.set(None);

        // Fetch the list of models
        let resp = match gloo_net::http::Request::get("/tama/v1/models").send().await {
            Ok(r) => r,
            Err(e) => {
                check_all_status.set(Some((false, format!("Failed to list models: {}", e))));
                check_all_busy.set(false);
                return;
            }
        };

        // Surface non-2xx HTTP responses
        if !resp.ok() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            check_all_status.set(Some((
                false,
                format!("Failed to list models: HTTP {} {}", status, body),
            )));
            check_all_busy.set(false);
            return;
        }

        // Parse using typed struct (NOT serde_json::Value::as_str() — that returns None for JSON numbers)
        let list: ModelsApiResponse = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                check_all_status
                    .set(Some((false, format!("Failed to parse models list: {}", e))));
                check_all_busy.set(false);
                return;
            }
        };

        let ids: Vec<i64> = list.models.iter().map(|m| m.id).collect();

        let total = ids.len();
        let mut ok_count = 0usize;
        let mut failed = Vec::<String>::new();
        for id in ids {
            let url = format!("/tama/v1/models/{}/refresh", id);
            match post_request(&url).send().await {
                Ok(r) if r.status() == 200 => ok_count += 1,
                Ok(r) => {
                    let text = r.text().await.unwrap_or_default();
                    failed.push(format!("{}: {}", id, text));
                }
                Err(e) => failed.push(format!("{}: {}", id, e)),
            }
        }

        if failed.is_empty() {
            check_all_status.set(Some((
                true,
                format!("Refreshed {}/{} models successfully.", ok_count, total),
            )));
        } else {
            check_all_status.set(Some((
                false,
                format!(
                    "Refreshed {}/{} models. Failures: {}",
                    ok_count,
                    total,
                    failed.join("; ")
                ),
            )));
        }
        check_all_busy.set(false);
        // Reconnect EventSource to pick up fresh model data from SSE stream
        connect_trigger.update(|n| *n += 1);
    });
```

**Alert banner placement (CRITICAL):** Place the alert banner **at the top level** of the `view!` macro, between the closing `</div>` of the page-header and the opening `{move || { ... }}` closure that contains `.grid-stats`. This ensures it is always visible, not nested inside the reactive closure.

Exact surrounding context:
```rust
// ... page-header div closes here ...
</div>

// Alert banner — always visible, outside reactive closure
{move || check_all_status.get().map(|(ok, msg)| {
    let cls = if ok { "alert alert--success" } else { "alert alert--error" };
    view! { <div class=cls>{msg}</div> }
})}

// .grid-stats reactive closure starts here
{move || {
    let buf = history.get();
    // ... rest of grid-stats, charts, models sections ...
```

**Note:** The alert banner persists until the next "Check all" invocation (no dismiss button, matches `models.rs` pattern).

**Important:**
- Use `post_request()` for all refresh POSTs (consistent with dashboard's existing `load_action`/`unload_action`)
- Use typed struct for parsing `/tama/v1/models` response — do NOT use `serde_json::Value::as_str()` (existing bug in `models.rs`)
- URL encoding is unnecessary for integer DB IDs but can be added for consistency with existing patterns

**Steps:**
- [ ] Add `pull_modal_open: RwSignal<bool>` signal to `Dashboard` component
- [ ] Add `use crate::components::modal::Modal;` import
- [ ] Add `use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};` import
- [ ] Add `use crate::utils::rw_signal_to_signal;` import (extracted in Task 1)
- [ ] Add `<Modal>` + `<PullQuantWizard>` block to the view
- [ ] Wire `on_complete` to close modal + increment `connect_trigger`
- [ ] Add `check_all_busy: RwSignal<bool>` and `check_all_status: RwSignal<Option<(bool, String)>>` signals
- [ ] Define `ModelsApiResponse` and `CheckAllModel` structs (local to this function or module, not pub)

- [ ] Implement `check_all_action` with typed struct parsing and `post_request()` for CSRF safety (see full implementation below)
- [ ] Add alert banner at top level of view! macro, between page-header closing `</div>` and `{move || { ... }}` closure
- [ ] Add `.page-header__actions` div with both buttons (Pull Model + Check all for updates) as direct child of page-header, outside the reactive closure
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(tama-web): add Pull Model and Check all for updates to dashboard"

**Acceptance criteria:**
- [ ] "Pull Model" button opens the `PullQuantWizard` modal
- [ ] On wizard completion, modal closes and EventSource reconnects
- [ ] "Check all for updates" button triggers sequential refresh POSTs
- [ ] Response is parsed using typed struct (not `as_str()` on JSON numbers)
- [ ] All refresh POSTs use `post_request()` for CSRF safety
- [ ] Alert banner appears at top level of view! macro (outside reactive closure)
- [ ] Both header buttons are in `.page-header__actions` div (outside reactive closure, always visible)
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings

---

## Task 5: Hide Models from sidebar

**Context:**
The Models page should no longer appear in the sidebar navigation (users can still access it directly via `/models` for Edit functionality). This is a simple navigation change.

**Files:**
- Modify: `crates/tama-web/src/components/sidebar.rs`

**What to implement:**
Remove the Models navigation link from the sidebar. Find this block:
```rust
<A href="/models" attr:class="sidebar-item" attr:data-tooltip="Models" on:click=move |_| mobile_open.set(false)>
    <span class="sidebar-item__icon">"📦"</span>
    <span class="sidebar-item__text">"Models"</span>
</A>
```
Delete it entirely (do not comment out).

**Do NOT change:** The route registration in `lib.rs` — the `/models` route must remain so Edit links and direct URL access still work.

**Steps:**
- [ ] Remove the `/models` `<A>` nav link from `crates/tama-web/src/components/sidebar.rs`
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "chore(tama-web): hide Models from sidebar navigation"

**Acceptance criteria:**
- [ ] Models link no longer appears in sidebar
- [ ] Route `/models` still registered in `lib.rs`
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings

---

## Task 6: Add unit tests for `inactive_models()`

**Context:**
The `inactive_models()` function (added in Task 2) needs test coverage matching the existing `active_models()` test suite. These are pure data tests that run on the host target (no WASM dependency).

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard.rs`

**What to implement:**
Add the following tests to the existing `#[cfg(test)]` module in `dashboard.rs`:

```rust
#[test]
fn inactive_models_filters_correctly() {
    let models = vec![
        ModelStatus { id: "a".into(), state: "ready".into(), ..Default::default() },
        ModelStatus { id: "b".into(), state: "idle".into(), ..Default::default() },
        ModelStatus { id: "c".into(), state: "loading".into(), ..Default::default() },
        ModelStatus { id: "d".into(), state: "failed".into(), ..Default::default() },
        ModelStatus { id: "e".into(), state: "unloading".into(), ..Default::default() },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 2);
    assert_eq!(inactive.iter().map(|m| &m.id).collect::<Vec<_>>(), vec!["b", "d"]);
}

#[test]
fn inactive_models_returns_empty_when_all_active() {
    let models = vec![
        ModelStatus { id: "a".into(), state: "ready".into(), ..Default::default() },
        ModelStatus { id: "b".into(), state: "loading".into(), ..Default::default() },
    ];

    assert!(inactive_models(&models).is_empty());
}

#[test]
fn inactive_models_returns_all_when_none_active() {
    let models = vec![
        ModelStatus { id: "a".into(), state: "idle".into(), ..Default::default() },
        ModelStatus { id: "b".into(), state: "failed".into(), ..Default::default() },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 2);
}

#[test]
fn inactive_models_returns_empty_for_empty_input() {
    let models: Vec<ModelStatus> = vec![];
    let inactive = inactive_models(&models);
    assert!(inactive.is_empty());
}

#[test]
fn inactive_models_treats_unknown_state_as_inactive() {
    let models = vec![
        ModelStatus { id: "a".into(), state: "migrating".into(), ..Default::default() },
        ModelStatus { id: "b".into(), state: "ready".into(), ..Default::default() },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 1);
    assert_eq!(inactive[0].id, "a");
}
```

**Steps:**
- [ ] Add all 5 tests to the `#[cfg(test)]` module in `crates/tama-web/src/pages/dashboard.rs`
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "test(tama-web): add unit tests for inactive_models()"

**Acceptance criteria:**
- [ ] All 5 tests pass
- [ ] `cargo test --package tama-web` passes with zero failures
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings

---

## Task 7: Final verification — format, lint, and test all

**Context:**
After all feature tasks are complete, run the full workspace checks to ensure no regressions were introduced.

**Files:**
- (No file changes — verification only)

**Steps:**
- [ ] Run `cargo fmt --all --check`
  - Did it pass? If not, run `cargo fmt --all` and re-check.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Commit with message: "chore: final verification — format, lint, test"

**Acceptance criteria:**
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes with zero warnings
- [ ] `cargo test --workspace` passes with zero failures
