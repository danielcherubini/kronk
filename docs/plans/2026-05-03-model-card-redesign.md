# Model Card Redesign Plan

**Goal:** Upgrade model rows on the dashboard and models page to card-style layout with left accent strip, badge pills, and icon-only secondary actions — inspired by a reference app while keeping our column-based information architecture.

**Architecture:** A new shared `ModelCard` component (`components/model_card.rs`) replaces the dashboard's `ModelRow` component and the models page's inline model row rendering. The component owns layout, accent strip coloring, badge pills, and action buttons. Both consumer pages pass raw model data and callbacks.

**Tech Stack:** Leptos 0.8, inline SVG icons, CSS custom properties from existing theme.

---

### Task 1: Create `ModelCard` component with helper functions and styles

**Context:**
This is the foundation — a new shared component that both the dashboard and models pages will use. It replaces duplicate rendering logic in `dashboard.rs` (`ModelRow` + helpers) and `models.rs` (inline model row). All badge/button helper functions move here from `dashboard.rs`, deduplicating the codebase.

**Files:**
- Create: `crates/tama-web/src/components/model_card.rs`
- Modify: `crates/tama-web/src/components/mod.rs`
- Modify: `crates/tama-web/style.css`

**What to implement:**

Create `components/model_card.rs` with:

1. **Helper functions** (copied from `dashboard.rs`, made `pub(crate)`):
   - `model_status_badge_class(state: &str) -> &'static str` — maps state string to CSS class (`ready` → `"badge badge-success"`, `loading` → `"badge badge-info"`, `unloading` → `"badge badge-warning"`, `failed` → `"badge badge-error"`, default → `"badge badge-muted"`)
   - `model_status_badge_label(state: &str) -> &'static str` — maps state to label (`ready` → `"Loaded"`, `loading` → `"Loading"`, `unloading` → `"Unloading"`, `failed` → `"Failed"`, default → `"Idle"`)
   - `model_action_button_class(state: &str) -> &'static str` — maps state to button class (`ready` → `"btn btn-danger btn-sm"`, `loading/unloading` → `"btn btn-secondary btn-sm"`, `failed` → `"btn btn-warning btn-sm"`, default → `"btn btn-success btn-sm"`)
   - `model_action_button_label(state: &str) -> &'static str` — maps state to label (`ready` → `"Unload"`, `loading` → `"Loading…"`, `unloading` → `"Unloading…"`, `failed` → `"Retry"`, default → `"Load"`)
   - `format_context_length(n: u32) -> String` — formats context length. Copied from `dashboard.rs`. Algorithm: if `n % 1024 == 0` → `{n/1024}k`; else if `n >= 1000 && n % 1000 == 0` → `{n/1000}k`; else → raw string.
   - `resolve_state(state: &str, loaded: Option<bool>) -> &str` — **NEW** helper. Resolves the effective state for badge/button logic. When `state` is non-empty, returns it as-is. When `state` is empty: `loaded == Some(true)` → `"ready"`, otherwise → `"idle"`. This preserves the models page's existing `loaded` boolean fallback behavior from `model_state_badge()`.

   **Note:** `model_display_name` is NOT moved to `model_card.rs`. It stays as a private helper in each page module (`dashboard.rs` and `models.rs`) because it operates on page-private types (`ModelStatus` and `ModelEntry`). This is known tech debt (minor DRY violation) but not addressed in this plan.

2. **`ModelCard` component** with the following props:
   ```rust
   #[component]
   pub fn ModelCard(
       id: String,                    // identifier for load/unload API calls
       db_id: Option<i64>,            // for edit URL construction
       display_name: String,          // model display name
       quant: Option<String>,         // quantization label (e.g. "Q6_K_XL")
       context_length: Option<u32>,   // raw context length
       backend: String,               // backend name for badge pill
       log_source: Option<String>,    // value for logs link URL
       state: String,                 // lifecycle state: ready, idle, loading, unloading, failed
       #[prop(default = None)]
       loaded: Option<bool>,          // fallback for state when state is empty string — see helpers
       #[prop(default = None)]
       enabled: Option<bool>,         // enabled/disabled state (models page only)
       #[prop(optional)]
       on_load: Option<Callback<String>>,
       #[prop(optional)]
       on_unload: Option<Callback<String>>,
       #[prop(optional)]
       load_busy: Option<RwSignal<bool>>,   // disables Load/Retry buttons when true
       #[prop(optional)]
       unload_busy: Option<RwSignal<bool>>, // disables Unload button when true
   ) -> impl IntoView
   ```

   **Note on Edit/Logs navigation:** The Edit and Logs actions are rendered as `<A>` (Leptos router) links, NOT callbacks. The component constructs URLs from its props:
   - Logs: `<A href="/logs?source={log_source}" ...>` — only renders when `log_source` is `Some`
   - Edit: `<A href="/models/{db_id_or_id}/edit" ...>` — uses `db_id` when `Some`, falls back to `id` string

   **Note on optional callbacks/URLs:** When `on_load` is `None`, the Load/Retry button is not rendered. When `on_unload` is `None`, the Unload button is not rendered. When `log_source` is `None`, the Logs icon is not rendered. When `db_id` is `None`, the Edit icon is still rendered using `id` as the URL parameter.

3. **Component view — two-line layout:**

   Line 1 (flex row): left accent strip (via CSS `border-left`) + inline server icon (16px SVG) + model name (flex-grow) + optional enabled badge + status badge + Load/Unload text button + icon-only Logs button + icon-only Edit button

   Line 2 (flex row, wraps): badge pills for quant (green-tinted `.badge-pill--quant`), context length (neutral `.badge-pill--context`), backend (muted `.badge-pill--backend`). Only renders when the value is `Some`.

   **Inline SVGs:** Use simple inline SVG strings (not external files or icon libraries). Define constants:
   - `SERVER_ICON` — a simple server/box glyph, 16x16
   - `LOGS_ICON` — a clipboard/document glyph, 14x14
   - `EDIT_ICON` — a pencil glyph, 14x14

   The server icon color is set via CSS class on the `<svg>` element (not inline `fill` attribute), so it picks up the accent strip color automatically.

4. **Busy signal handling:** Two independent busy signals are supported:
   - `load_busy`: when `Some` and `true`, disables Load and Retry buttons
   - `unload_busy`: when `Some` and `true`, disables Unload button
   This preserves the current dashboard behavior where a pending load doesn't disable the Unload button (and vice versa). The signals are NOT owned by the component — they're passed in from the parent.

5. **Action button logic:** The component first resolves the effective state via `resolve_state(&state, loaded)`. All badge and button logic uses this resolved state, NOT the raw `state` prop. This ensures the `loaded` fallback works for both badges and buttons consistently.
   - `effective_state == "ready"` → show Unload button (btn-danger), dispatches `on_unload` (hidden if `on_unload` is `None`)
   - `effective_state == "loading" | "unloading"` → show disabled secondary button with "Loading…" / "Unloading…"
   - `effective_state == "failed"` → show Retry button (btn-warning), dispatches `on_load` (hidden if `on_load` is `None`)
   - default (idle, etc.) → show Load button (btn-success), dispatches `on_load` (hidden if `on_load` is `None`)
   - **Logs:** Rendered as `<A>` (Leptos router link) styled as `.btn-icon` with `title="View backend logs"`, `href="/logs?source={log_source}"`. Not rendered when `log_source` is `None`.
   - **Edit:** Rendered as `<A>` (Leptos router link) styled as `.btn-icon` with `title="Edit model"`, `href="/models/{id}/edit"` where `id` is `db_id` (as string) when `Some`, otherwise the `id` prop. Always rendered.
   - Both Edit and Logs use `<A>` links (not callbacks) to preserve SPA client-side navigation, matching the current `ModelRow` behavior in dashboard.rs.

6. **Tests** — migrate from `dashboard.rs`:
   - `test_model_status_badge_class_uses_success_when_ready`
   - `test_model_status_badge_class_uses_muted_when_idle`
   - `test_model_status_badge_label_distinguishes_ready_and_idle`
   - `test_model_action_button_class_uses_danger_when_ready`
   - `test_model_action_button_class_uses_success_when_idle`
   - `test_model_action_button_class_uses_secondary_when_loading`
   - `test_format_context_length` tests: `1024 → "1k"`, `2048 → "2k"`, `256000 → "256k"`, `8192 → "8k"`, `1000 → "1k"` (decimal-k branch), `999 → "999"` (non-k branch)
   - New: `test_model_card_renders_with_all_props`, `test_model_card_renders_without_optional_props` — these are compile-only smoke tests verifying the component accepts all props and compiles with only required props
   - New: `test_model_card_shows_enabled_badge_when_some_true`, `test_model_card_shows_disabled_badge_when_some_false`, `test_model_card_hides_enabled_badge_when_none`
   - New: `test_resolve_state_fallback_to_loaded_true`, `test_resolve_state_fallback_to_idle`, `test_resolve_state_passthrough_when_non_empty`

**CSS changes in `style.css`:**

1. **Add to existing `:root` block** (around line 22, after `--accent-cyan`):
   ```css
   --accent-orange: #fb923c;
   ```
   This adds the orange accent variable needed for the unloading state. Do NOT create a new `:root` block — add to the existing one.

2. **Add after existing `.badge` styles** (around line 695):

/* Badge pills — small rounded pills for model card metadata */
.badge-pill {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: 9999px;
  font-size: 0.7rem;
  font-weight: 500;
  line-height: 1.4;
  white-space: nowrap;
}

.badge-pill--quant {
  background: rgba(63, 185, 80, 0.12);
  color: var(--accent-green);
}

.badge-pill--context {
  background: rgba(139, 148, 158, 0.12);
  color: #8b949e;
}

.badge-pill--backend {
  background: rgba(139, 148, 158, 0.08);
  color: #6e7681;
}

.badge-pill--enabled {
  background: rgba(63, 185, 80, 0.12);
  color: var(--accent-green);
}

.badge-pill--disabled {
  background: rgba(210, 153, 34, 0.12);
  color: var(--accent-yellow);
}

/* Model list card — horizontal two-line card for dashboard and models page */
.model-list-card {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
  padding: 0.6rem 0.75rem 0.6rem 0.5rem;
  margin-bottom: 0.25rem;
  border-radius: 0.5rem;
  border-left: 3px solid #374151;
  transition:
    border-color var(--transition-fast),
    box-shadow var(--transition-fast),
    background var(--transition-fast);
}

.model-list-card:hover {
  border-color: var(--border-hover);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
}

/* Accent strip states — applied via state class on .model-list-card */
.model-list-card--ready {
  border-left-color: var(--accent-green);
  box-shadow: -2px 0 8px rgba(63, 185, 80, 0.25);
}

.model-list-card--loading {
  border-left-color: var(--accent-yellow);
  box-shadow: -2px 0 6px rgba(210, 153, 34, 0.2);
}

.model-list-card--unloading {
  border-left-color: var(--accent-orange);
}

.model-list-card--failed {
  border-left-color: var(--accent-red);
  box-shadow: -2px 0 6px rgba(248, 81, 73, 0.2);
}

/* Line 1 — name, status badge, actions */
.model-list-card__line1 {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  flex-wrap: nowrap;
}

.model-list-card__icon {
  flex-shrink: 0;
  width: 16px;
  height: 16px;
  color: var(--text-muted);
}

.model-list-card--ready .model-list-card__icon {
  color: var(--accent-green);
}

.model-list-card--loading .model-list-card__icon {
  color: var(--accent-yellow);
}

.model-list-card--failed .model-list-card__icon {
  color: var(--accent-red);
}

.model-list-card__name {
  font-size: 0.875rem;
  font-weight: 600;
  color: var(--text-primary);
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex-grow: 1;
}

.model-list-card__actions {
  display: flex;
  align-items: center;
  gap: 0.375rem;
  flex-shrink: 0;
  margin-left: auto;
}

/* Line 2 — badge pills */
.model-list-card__line2 {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  flex-wrap: wrap;
  padding-left: 1.125rem; /* align with text after icon */
}

/* Icon-only action buttons */
.model-list-card .btn-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  padding: 0;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition:
    color var(--transition-fast),
    border-color var(--transition-fast),
    background var(--transition-fast);
}

.model-list-card .btn-icon:hover {
  color: var(--text-primary);
  border-color: var(--border-hover);
  background: rgba(255, 255, 255, 0.04);
}

.model-list-card .btn-icon:disabled {
  opacity: 0.3;
  cursor: not-allowed;
}

/* Responsive — wrap line 1 on narrow screens */
@media (max-width: 900px) {
  .model-list-card__line1 {
    flex-wrap: wrap;
  }

  .model-list-card__name {
    min-width: 120px;
    max-width: 60%;
  }
}
```

**Steps:**
- [ ] Create `crates/tama-web/src/components/model_card.rs` with all helper functions, the `ModelCard` component, and inline SVG constants
- [ ] Add `pub mod model_card;` to `crates/tama-web/src/components/mod.rs`
- [ ] Add all CSS from above to `crates/tama-web/style.css` (after existing `.badge` styles, around line 695)
- [ ] Write unit tests for helper functions in `model_card.rs` (copied from dashboard.rs tests)
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: create shared ModelCard component with badge pills and accent strip"

**Acceptance criteria:**
- [ ] `ModelCard` component compiles without errors
- [ ] All helper function tests pass (badge class, badge label, button class, context length formatting)
- [ ] CSS classes `.model-list-card`, `.badge-pill`, `.btn-icon` exist in `style.css`
- [ ] Component is exported from `components/mod.rs`

---

### Task 2: Migrate dashboard to use `ModelCard`

**Context:**
The dashboard currently has a `ModelRow` component and several helper functions. This task replaces them with the shared `ModelCard` component. The `ModelRow` component is removed, and the dashboard's `normalize_models()` is simplified to pass raw data to `ModelCard` instead of pre-computed `ModelDisplayData`.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard.rs`

**What to implement:**

1. **Remove** the following from `dashboard.rs`:
   - `ModelRow` component (entire `#[component] fn ModelRow(...)`)
   - `ModelDisplayData` struct
   - Helper functions: `model_status_badge_class`, `model_status_badge_label`, `model_action_button_class`, `model_action_button_label`, `format_context_length` — these are now in `model_card.rs`
   - `normalize_models()` function — replace with direct iteration over `ModelStatus`

   **Keep** `model_display_name` in `dashboard.rs` — it operates on the private `ModelStatus` type and cannot be shared.

2. **Update imports** — add `use crate::components::model_card::ModelCard;`, remove unused imports

3. **Replace `ModelRow` usage** in the Active and Inactive sections. Replace the `normalize_models()` call + `ModelRow` rendering with direct iteration:

   For each model in `active` / `inactive`:
   ```rust
   <ModelCard
       id=m.id.clone()
       db_id=m.db_id
       display_name=model_display_name(&m)
       quant=m.quant.clone()
       context_length=m.context_length
       backend=m.backend.clone()
       log_source=Some(format!("{}_{}", m.backend, m.id))
       state=m.state.clone()
       loaded=None
       enabled=None
       on_load=Some(on_load_cb)
       on_unload=Some(on_unload_cb)
       load_busy=Some(load_busy)
       unload_busy=Some(unload_busy)
   />
   ```

   Note: `on_edit` and `on_logs` are NOT callbacks — the component constructs Edit and Logs URLs internally from `db_id`/`id` and `log_source` respectively, rendering them as `<A>` links.

   Note: The `model_display_name()` function stays in `dashboard.rs` as a private helper — it was NOT migrated to `model_card.rs` because it operates on the private `ModelStatus` type.

   **Important — preserve sorting:** The models must be sorted by `id` before rendering, as the old `normalize_models()` did. Sort `active` and `inactive` vectors before iterating:
   ```rust
   let mut active_sorted = active.clone();
   active_sorted.sort_by(|a, b| a.id.cmp(&b.id));
   ```
   Then iterate over `active_sorted` instead of `active`. Apply the same pattern for `inactive_sorted`.

4. **Callback wiring:** The `on_load` and `on_unload` callbacks dispatch to `load_action` and `unload_action` respectively. The component constructs Edit and Logs URLs internally from `db_id`/`id` and `log_source` — no edit/logs callbacks needed.

5. **Update tests:**
   - Remove migrated tests (they're now in `model_card.rs`)
   - Keep `active_models`, `inactive_models`, `merge_samples`, `backfill_metrics`, `format_number`, and SSE-specific tests
   - The `model_display_name` function is NOT a helper that was migrated — it stays in dashboard.rs

**Steps:**
- [ ] Remove `ModelRow` component, `ModelDisplayData`, and migrated helper functions from `dashboard.rs`
- [ ] Add `ModelCard` import and replace `ModelRow` usage in Active/Inactive sections
- [ ] Update callbacks to wire through `ModelCard` props
- [ ] Remove migrated tests from `dashboard.rs` test module
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: migrate dashboard to shared ModelCard component"

**Acceptance criteria:**
- [ ] Dashboard compiles without errors
- [ ] No `ModelRow`, `ModelDisplayData`, or migrated helpers remain in `dashboard.rs`
- [ ] Dashboard uses `ModelCard` for both Active and Inactive sections
- [ ] All remaining dashboard tests pass
- [ ] The migrated tests now live in `model_card.rs`

---

### Task 3: Migrate models page to use `ModelCard`

**Context:**
The models page currently renders model rows inline (not a separate component). This task replaces the inline rendering with the shared `ModelCard` component, enabling consistent look-and-feel across both pages.

**Files:**
- Modify: `crates/tama-web/src/pages/models.rs`

**What to implement:**

1. **Add import:** `use crate::components::model_card::ModelCard;`

2. **Remove** `model_state_badge()` helper function (the `ModelCard` component handles badge rendering internally)

3. **Replace inline model row rendering** inside the `Some(data)` branch. Current code:
   ```rust
   data.models.into_iter().map(|m| {
       // ... inline <div class="model-row card"> with badges and buttons
   })
   ```

   Replace with:
   ```rust
   data.models.into_iter().map(|m| {
       view! {
           <ModelCard
               id=m.id.to_string()
               db_id=Some(m.id)
               display_name=model_display_name(&m)
               quant=m.quant.clone()
               context_length=None
               backend=m.backend.clone()
               log_source=Some(m.backend.clone())
               state=m.state.clone()
               loaded=Some(m.loaded)
               enabled=Some(m.enabled)
               on_load=Some(on_load_cb)
               on_unload=Some(on_unload_cb)
               load_busy=None
               unload_busy=None
           />
       }
   })
   ```

   Note: `on_edit` and `on_logs` are NOT callbacks — the component constructs Edit and Logs URLs internally from `db_id`/`id` and `log_source` respectively, rendering them as `<A>` links.

   Note: `model_display_name()` in `models.rs` already exists as a private helper — keep it.

4. **Remove** the `enabled_class` and `model_state_badge` logic from the view (the component handles this).

5. **The models page does NOT use a shared busy signal** for load/unload — it uses `refresh.update()` after each action. The `load_busy` and `unload_busy` props are `None`.

**Steps:**
- [ ] Add `ModelCard` import to `models.rs`
- [ ] Remove `model_state_badge()` helper function
- [ ] Replace inline model row rendering with `ModelCard` usage
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: migrate models page to shared ModelCard component"

**Acceptance criteria:**
- [ ] Models page compiles without errors
- [ ] No inline model row rendering remains in `models.rs`
- [ ] Models page uses `ModelCard` with `enabled=Some(m.enabled)`
- [ ] All tests pass

---

### Task 4: Clean up deprecated CSS

**Context:**
After both consumers have migrated to `.model-list-card`, the old `.model-row` CSS is dead code. This task removes it and verifies no other references exist.

**Files:**
- Modify: `crates/tama-web/style.css`

**What to implement:**

1. **Audit CSS references** — search `style.css` for `.model-row` and verify it's only used by the old `.model-row` selectors (lines ~1521-1610). Confirm no other components reference it.

2. **Remove** the following CSS selectors (around lines 1521-1610):
   - `.model-row`
   - `.model-row:hover`
   - `.model-row__name`
   - `.model-row__meta`
   - `.model-row__backend`
   - `.model-row__badge`
   - `.model-row__actions`
   - `.model-row .badge`
   - `@media (max-width: 900px) { .model-row ... }` block

3. **Verify** the existing `.model-card` CSS (lines ~1087-1152) is still used by `backend_card.rs` and should NOT be removed.

4. **Check** for any `class="model-row"` references in Rust source files. If found, this task is blocked and the consumer must be updated first.

**Steps:**
- [ ] Run `grep -rn "model-row" crates/tama-web/src/` to confirm no Rust source references remain
- [ ] If references exist, update them to `model-list-card` or the appropriate class
- [ ] Remove `.model-row` CSS selectors from `style.css`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "chore: remove deprecated .model-row CSS"

**Acceptance criteria:**
- [ ] No `model-row` class references in Rust source files
- [ ] `.model-row` CSS selectors removed from `style.css`
- [ ] `.model-card` CSS (used by backend_card.rs) is preserved
- [ ] Build succeeds

---

### Task 5: Run full workspace check

**Context:**
Final verification that the entire workspace builds, passes tests, and has no clippy warnings or formatting issues.

**Files:**
- None (verification only)

**What to implement:**
Nothing new. This is a verification task.

**Steps:**
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run
- [ ] Run `cargo build --release --package tama-web`
  - Did it succeed? If not, fix and re-run

**Note:** This is a verification-only task. Do NOT create an empty commit. If any of the checks above fail, fix the issues in the appropriate task files and re-run from that task. Only commit if there are actual code changes needed.

**Acceptance criteria:**
- [ ] `cargo check --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --all` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo build --release --package tama-web` passes
