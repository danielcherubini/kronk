# Move Self-Update to Updates Center

**Goal:** Move the Koji application self-update UI from the sidebar to the `/updates` page, keeping only a minimal read-only version indicator in the sidebar.

**Architecture:** The full self-update flow (version check, update button, confirmation dialog, progress overlay, SSE streaming, restart polling) moves into a new `SelfUpdateSection` component rendered on the `/updates` page. The two shared streaming functions (`stream_update_events`, `poll_for_restart`) are extracted to a utility module. The sidebar is stripped of all self-update logic and replaced with a minimal clickable version text that links to `/updates`.

**Tech Stack:** Rust, Leptos (WASM), Axum, SSE, CSS

**Execution order:** Tasks must be done in order: 1 → 2 → 3 → 4 → 5. Task 2 cannot compile without Task 1's utility module. Task 3 depends on Task 2. Tasks 4 and 5 are independent but semantically belong together (sidebar cleanup + CSS cleanup).

---

### Task 1: Create shared self-update utility module

**Context:**
Currently `stream_update_events()` and `poll_for_restart()` are free functions inside `sidebar.rs` (~140 lines combined). They will be needed by both the sidebar (for its minimal version check) and the new `SelfUpdateSection` component. Extracting them prevents duplication and makes future maintenance easier.

**Files:**
- Create: `crates/koji-web/src/utils/self_update.rs`
- Modify: `crates/koji-web/src/utils.rs` (add `pub mod self_update;`)

**What to implement:**

A new module `utils/self_update.rs` with two public async functions and their helper types. The functions are direct copies from the current `sidebar.rs` implementation (lines 273–412, the two async functions at the bottom of the file), modified to remove the `LogPayload`/`StatusPayload` structs (which become internal to the module) and use Leptos signals directly.

The module needs:

```rust
use leptos::prelude::*;
use gloo_net::eventsource::futures::EventSource;
use futures_util::StreamExt;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct LogPayload {
    line: String,
}

#[derive(Debug, Clone, Deserialize)]
struct StatusPayload {
    status: String,
    #[serde(default)]
    error: Option<String>,
}

/// Stream update events via SSE, matching the pattern from `job_log_panel.rs`.
pub async fn stream_update_events(
    update_status: RwSignal<String>,
    update_in_progress: RwSignal<bool>,
    update_available: RwSignal<bool>,
    current_version: RwSignal<String>,
    latest_version: RwSignal<String>,
);

/// Poll `/api/self-update/check` every 2 seconds until the server
/// responds with a new version, or give up after 5 attempts.
pub async fn poll_for_restart(
    update_status: RwSignal<String>,
    update_in_progress: RwSignal<bool>,
    update_available: RwSignal<bool>,
    current_version: RwSignal<String>,
    latest_version: RwSignal<String>,
);
```

The `LogPayload` and `StatusPayload` structs are private to this module. The function bodies are identical to the current sidebar implementation — copy them verbatim from `sidebar.rs` lines 273–416, adjusting visibility of the helper structs to private.

In `utils.rs`, add:
```rust
pub mod self_update;
```

**Steps:**
- [ ] Create `crates/koji-web/src/utils/self_update.rs` with the two functions and their private helper structs (copy from sidebar.rs lines 273–416, adjusting visibility)
- [ ] Add `pub mod self_update;` to `crates/koji-web/src/utils.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package koji-web`
  - Did it succeed? If not, fix import issues and re-run before continuing.
- [ ] Commit with message: "chore(web): extract self-update streaming to shared utils module"

**Acceptance criteria:**
- [ ] `crates/koji-web/src/utils/self_update.rs` exists with `stream_update_events` and `poll_for_restart` as public async functions
- [ ] `utils.rs` exports the module via `pub mod self_update;`
- [ ] The two functions have identical behavior to the sidebar implementation (same SSE subscription, same event handling, same polling logic)
- [ ] `cargo build --package koji-web` succeeds

---

### Task 2: Create SelfUpdateSection component

**Context:**
The main self-update UI needs a new home on the `/updates` page. This component encapsulates all self-update state, API calls, error handling, and UI (version display, update button, confirmation dialog, progress overlay). It follows the existing pattern of `backup_section.rs`, `general_section.rs`, etc.

**Files:**
- Create: `crates/koji-web/src/components/self_update_section.rs`
- Modify: `crates/koji-web/src/components/mod.rs` (add `pub mod self_update_section;`)

**What to implement:**

A new `#[component] pub fn SelfUpdateSection() -> impl IntoView`.

**Signals:**
```rust
let current_version = RwSignal::new(String::new());
let update_available = RwSignal::new(false);
let latest_version = RwSignal::new(String::new());
let update_in_progress = RwSignal::new(false);
let update_status = RwSignal::new(String::new());
let show_update_confirm = RwSignal::new(false);
let check_error = RwSignal::new(Option::<String>::None);
```

**On mount:** GET `/api/self-update/check` with error handling:
- On success: populate `current_version`, `update_available`, `latest_version`, clear `check_error`
- On failure (network error, 502, etc.): set `check_error` to an error message string

**UI states (four distinct states, no silent empty):**

1. **Loading** (initial check in flight): show "Checking for updates…" with a spinner
2. **Error** (`check_error.is_some()`): show error message + "Retry" button
3. **Up to date** (check succeeded, `update_available` is false): show `v{current_version}` — "Up to date"
4. **Update available** (check succeeded, `update_available` is true): show `v{current} → v{latest}` + Update button

When `update_in_progress` is true:
- Show inline progress display with spinner and status text **inside the card** (NOT an overlay)
- The confirmation dialog is a page-scoped overlay (outside the card), shown BEFORE the update starts
- No progress overlay — the inline progress within the card is sufficient since SelfUpdateSection is a page element, not a sidebar widget

**UI structure:**
```rust
view! {
    <div class="self-update-section">
        <h2 class="section__title">Koji</h2>

        // Loading state (initial check in flight)
        {move || (current_version.with(|v| v.is_empty()) && check_error.get().is_none() && !update_in_progress.get()).then(|| view! {
            <div class="self-update-progress">
                <div class="self-update-spinner"></div>
                <span>"Checking for updates…"</span>
            </div>
        })}

        // Inline progress during update
        {move || update_in_progress.get().then(|| view! {
            <div class="self-update-progress">
                <div class="self-update-spinner"></div>
                <span>{move || update_status.get()}</span>
            </div>
        })}

        // Error state with retry (only when not in progress)
        {move || (!update_in_progress.get() && check_error.get().is_some()).then(|| view! {
            <div class="self-update-error">
                <span>{move || check_error.get().clone().unwrap_or_default()}</span>
                <button class="btn btn-ghost" on:click=retry_check>"Retry"</button>
            </div>
        })}

        // Normal state (no error, not in progress, not loading)
        {move || (!update_in_progress.get() && check_error.get().is_none() && !current_version.with(|v| v.is_empty())).then(|| view! {
            <div class="self-update-info">
                <span class="self-update-version">
                    {move || {
                        let cv = current_version.get();
                        if update_available.get() && !cv.is_empty() {
                            format!("v{} → v{}", cv, latest_version.get())
                        } else {
                            format!("v{}", cv)
                        }
                    }}
                </span>
                {move || (update_available.get() && !update_in_progress.get()).then(|| view! {
                    <button class="btn btn-primary" disabled=move || update_in_progress.get()
                        on:click=move |_| show_update_confirm.set(true)>
                        "Update"
                    </button>
                })}
            </div>
        })}
    </div>

    // Confirmation dialog (page-scoped overlay, shown BEFORE update starts)
    {move || show_update_confirm.get().then(|| view! {
        <div class="update-confirm-overlay">
            <div class="update-confirm-dialog">
                <p>{format!("Update Koji to v{}?", latest_version.get())}</p>
                <p class="update-confirm-note">"Koji will restart after updating."</p>
                <div class="update-confirm-actions">
                    <button class="btn btn-secondary" on:click=move |_| show_update_confirm.set(false)>"Cancel"</button>
                    <button class="btn btn-primary" on:click=confirm_update>"Update"</button>
                </div>
            </div>
        </div>
    })}
}
```

**Handlers:**
- `confirm_update`: Same as sidebar's current handler — POST `/api/self-update/update`, then call `stream_update_events()`. Handle 409 conflict (already in progress) by setting `check_error` to "An update is already in progress."
- `retry_check`: Clear `check_error`, re-run the initial GET check

**Imports needed:**
```rust
use leptos::prelude::*;
use gloo_net::http::Request;
use wasm_bindgen_futures::spawn_local;
use crate::utils::self_update::{stream_update_events, poll_for_restart};
```

In `components/mod.rs`, add:
```rust
pub mod self_update_section;
```

**Steps:**
- [ ] Create `crates/koji-web/src/components/self_update_section.rs` with the component, signals, handlers, and UI
- [ ] Add `pub mod self_update_section;` to `crates/koji-web/src/components/mod.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package koji-web`
  - Did it succeed? If not, fix import issues and re-run before continuing.
- [ ] Commit with message: "feat(web): add SelfUpdateSection component for /updates page"

**Acceptance criteria:**
- [ ] `SelfUpdateSection` component exists and compiles
- [ ] On mount, GETs `/api/self-update/check` and populates version signals
- [ ] Shows four distinct states: loading, error (with retry), up-to-date, update-available (with button)
- [ ] Clicking "Update" triggers the full flow: confirm → POST → SSE streaming → restart polling
- [ ] 409 conflict ("already in progress") is handled with an error message
- [ ] SSE streaming and restart polling use the shared utility functions from `utils::self_update`

---

### Task 3: Update the /updates page to render SelfUpdateSection

**Context:**
The `/updates` page currently shows only Backends and Models sections. We need to render the new `SelfUpdateSection` at the top of the page, before the "Backends" section.

**Files:**
- Modify: `crates/koji-web/src/pages/updates.rs`

**What to implement:**

Add import and render `SelfUpdateSection` at the top of the page's view:

```rust
use crate::components::self_update_section::SelfUpdateSection;
```

In the view, insert `<SelfUpdateSection />` right after the header row (after the "Check Now" button and last-checked time), before the first `<section class="updates-section">`:

```rust
view! {
    <div class="page updates-page">
        <h1 class="page__title">"Updates Center"</h1>

        <div class="updates-header">
            // ... existing header with Check Now button and last-checked time
        </div>

        {move || error.get().map(|e| view! {
            <div class="error-banner">{e}</div>
        })}

        // NEW: Self-update section for the Koji application itself
        <SelfUpdateSection />

        // Existing Backends section
        <section class="updates-section">
            <h2 class="section__title">"Backends"</h2>
            // ... rest unchanged
```

**Steps:**
- [ ] Add `use crate::components::self_update_section::SelfUpdateSection;` import to `updates.rs`
- [ ] Insert `<SelfUpdateSection />` between the error banner and the Backends section
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package koji-web`
  - Did it succeed? If not, fix import issues and re-run before continuing.
- [ ] Commit with message: "feat(web): render SelfUpdateSection on /updates page"

**Acceptance criteria:**
- [ ] `/updates` page shows the SelfUpdateSection card at the top
- [ ] The Koji self-update section is visually separated from the Backends/Models sections below it

---

### Task 4: Clean up sidebar — remove self-update logic, add minimal version indicator

**Context:**
The sidebar currently contains all self-update logic: signals, API calls, handlers, overlays, and streaming functions. All of this moves to `SelfUpdateSection` (Task 2) and the shared utility (Task 1). The sidebar is left with a minimal read-only version indicator that links to `/updates`.

**Files:**
- Modify: `crates/koji-web/src/components/sidebar.rs`

**What to remove (delete these sections from sidebar.rs):**

1. **Remove self-update signals** (the block starting with `// Self-update signals` through `show_update_confirm`):
```rust
// Self-update signals
let current_version = RwSignal::new(String::new());
let update_available = RwSignal::new(false);
let latest_version = RwSignal::new(String::new());
let update_in_progress = RwSignal::new(false);
let update_status = RwSignal::new(String::new());
let show_update_confirm = RwSignal::new(false);
```

2. **Remove the self-update check on mount** (the `leptos::task::spawn_local(async move { ... })` block that GETs `/api/self-update/check`)

3. **Remove `confirm_update` closure** (the entire `let confirm_update = move |_| { ... }` block)

4. **Remove the `<div class="sidebar-version">` block** — the entire version badge with update button:
```rust
<div class="sidebar-version">
    <span class="sidebar-version__text">...</span>
    {move || update_available.get().then(|| view! { ... })}
</div>
```

5. **Remove the confirmation dialog overlay** — the `{move || show_update_confirm.get().then(|| view! { ... })}` block

6. **Remove the progress overlay** — the `{move || update_in_progress.get().then(|| view! { ... })}` block

7. **Remove the `LogPayload` and `StatusPayload` structs** (lines 9–20) — these are now in `utils/self_update.rs`

8. **Remove the `stream_update_events()` function** (lines 273–369)

9. **Remove the `poll_for_restart()` function** (lines 371–416)

**What to keep:**
- `collapsed`, `mobile_open` signals
- `update_badge_visible` signal and its Effect (checks `/api/updates` for backend/model updates — this is NOT self-update)
- localStorage persistence for collapsed state
- All navigation links
- Collapse toggle button
- Mobile hamburger/close buttons

**What to add (minimal version indicator):**

Replace the removed `<div class="sidebar-version">` with:
```rust
<div class="sidebar-version-minimal">
    <A href="/updates" on:click=move |_| mobile_open.set(false)>
        {move || {
            let cv = current_version.get();
            if !cv.is_empty() {
                format!("v{}", cv)
            } else {
                String::new()
            }
        }}
    </A>
</div>
```

This requires adding a single lightweight GET check on mount:
```rust
leptos::task::spawn_local(async move {
    if let Ok(resp) = gloo_net::http::Request::get("/api/self-update/check").send().await {
        if let Ok(data) = resp.json::<serde_json::Value>().await {
            if let Some(v) = data["current_version"].as_str() {
                current_version.set(v.to_string());
            }
        }
    }
});
```

With `current_version: RwSignal::new(String::new())` as the only self-update-related signal.

**Required imports:** Keep `gloo_net::http::Request`, add `leptos_router::components::A` (if not already imported).

**Steps:**
- [ ] Remove all self-update logic from sidebar.rs (signals, API calls, handlers, overlays, streaming functions, helper structs)
- [ ] **Remove unused imports:** `use futures_util::StreamExt;`, `use gloo_net::eventsource::futures::EventSource;`, `use serde::Deserialize;` — these were only used by the removed self-update code
- [ ] **Keep these imports:** `use gloo_net::http::Request;` (needed for minimal version check), `use leptos_router::components::A;` (needed for version link), `use web_sys::window;` (needed for localStorage)
- [ ] Add minimal version indicator with single GET check on mount and `<A href="/updates">` link
- [ ] Keep `update_badge_visible` logic (backend/model update badge — NOT self-update)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package koji-web`
  - Did it succeed? If not, fix import issues and re-run before continuing.
- [ ] Commit with message: "refactor(web): move self-update from sidebar to SelfUpdateSection"

**Acceptance criteria:**
- [ ] Sidebar no longer has self-update button, confirmation dialog, or progress overlay
- [ ] Sidebar no longer calls `stream_update_events` or `poll_for_restart`
- [ ] Sidebar still shows the "!" badge on Updates nav item (backend/model updates)
- [ ] Sidebar shows a minimal clickable version text (e.g., "v1.36.1") that links to `/updates`
- [ ] The version check is a single GET call on mount only (no SSE, no polling)
- [ ] `cargo build --package koji-web` succeeds

---

### Task 5: Update CSS — remove sidebar-version rules, add self-update-section rules

**Context:**
The CSS needs cleanup: remove all sidebar-version related rules (they're no longer used), and add new styles for the SelfUpdateSection card and the minimal version indicator.

**Files:**
- Modify: `crates/koji-web/style.css`

**What to remove:**
Delete these CSS rule blocks (approximately lines 255–308):
- `.sidebar-version { ... }`
- `.sidebar--collapsed .sidebar-version { ... }`
- `.sidebar-version__text { ... }`
- `.sidebar--collapsed .sidebar-version__text { ... }`
- `.sidebar-update-btn { ... }`
- `.sidebar-update-btn:hover { ... }`
- `.sidebar-update-btn:disabled { ... }`
- `.sidebar--collapsed .sidebar-update-btn { ... }`

**What to add:**

Add these CSS rules (insert near the existing sidebar styles, before the update-confirm/progress overlay styles at line ~308):

```css
/* Minimal version indicator in sidebar footer */
.sidebar-version-minimal {
  padding: 4px 8px;
  margin: 4px 0;
}

.sidebar-version-minimal a {
  font-size: 0.75rem;
  color: var(--text-muted, #94a3b8);
  text-decoration: none;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: 4px;
  transition: background-color 0.15s ease;
}

.sidebar-version-minimal a:hover {
  background: rgba(255, 255, 255, 0.05);
  color: var(--text-primary, #e2e8f0);
}

/* Self-update section card on /updates page */
.self-update-section {
  background: var(--bg-secondary, #1e293b);
  border: 1px solid var(--border, #334155);
  border-radius: 8px;
  padding: 16px;
  margin-bottom: 24px;
}

.self-update-section .section__title {
  margin: 0 0 8px 0;
  font-size: 1rem;
  color: var(--text-primary, #e2e8f0);
}

.self-update-info {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 4px;
}

.self-update-version {
  font-size: 0.9rem;
  color: var(--text-muted, #94a3b8);
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.self-update-progress {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 8px;
}

.self-update-spinner {
  width: 16px;
  height: 16px;
  border: 2px solid var(--border, #334155);
  border-top-color: var(--primary, #3b82f6);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

.self-update-error {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 8px;
  color: var(--error, #ef4444);
  font-size: 0.9rem;
}
```

**What to keep unchanged:**
- All `.update-confirm-overlay`, `.update-confirm-dialog`, `.update-confirm-note`, `.update-confirm-actions` rules (still used by SelfUpdateSection)
- All `.update-progress-overlay`, `.update-progress-dialog`, `.update-progress-spinner` rules (still used by SelfUpdateSection)
- `@keyframes spin` animation (still needed for the inline spinner)

**Steps:**
- [ ] Delete sidebar-version related CSS rules (~50 lines, approximately lines 255–308)
- [ ] Add new CSS rules for `.sidebar-version-minimal` and `.self-update-section`
- [ ] Run `cargo fmt --all` (CSS files are not affected by Rust formatter, but good practice)
- [ ] Verify the build still succeeds: `cargo build --package koji-web`
- [ ] Commit with message: "style(web): update CSS for self-update section, remove sidebar-version rules"

**Acceptance criteria:**
- [ ] All `.sidebar-version`, `.sidebar--collapsed .sidebar-version`, `.sidebar-version__text`, `.sidebar-update-btn` CSS rules are removed
- [ ] New `.sidebar-version-minimal` styles exist and work
- [ ] New `.self-update-section`, `.self-update-info`, `.self-update-version`, `.self-update-progress`, `.self-update-spinner`, `.self-update-error` styles exist
- [ ] Existing overlay styles (`update-confirm-overlay`, etc.) are preserved
- [ ] `@keyframes spin` is preserved

---

## Verification

After all tasks are complete:

1. **Build:** `cargo build --workspace` succeeds
2. **Format:** `cargo fmt --all` produces no changes
3. **Lint:** `cargo clippy --workspace -- -D warnings` passes
4. **Tests:** `cargo test --workspace` passes
5. **Visual check:**
   - `/updates` page shows SelfUpdateSection card at the top with Koji version
   - Sidebar shows minimal version text only (no update button)
   - Clicking sidebar version navigates to `/updates`
   - "!" badge on Updates nav item still works (backend/model updates)
   - Update flow works: click Update → confirm → progress → restart
