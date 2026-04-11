# Collapsible Left Sidebar Navigation Plan

**Goal:** Replace the sticky top navbar with a collapsible left sidebar, fixing page-level sidebars being obscured by the topbar and establishing a standard admin-panel layout.

**Architecture:** The current layout is `<Nav/> (sticky topbar) + <main>`. This becomes `<Sidebar/> (fixed left) + <main> (with left margin)`. The sidebar defaults to expanded (icons + labels, 200px), collapses to icons only (56px), persists state in localStorage, and becomes a mobile overlay on narrow screens. Config and Model Editor pages retain their inner section-navigation sidebars (3-column layout).

**Tech Stack:** Leptos 0.7 (CSR), custom CSS design system (dark theme), `web_sys::window()` for localStorage, `wasm_bindgen` for JS interop.

---

### Task 1: Create Sidebar CSS + Sidebar Component

**Context:**
The app currently has a topbar (`components/nav.rs` + `.topbar` CSS). We need to create the new sidebar component and its CSS styles. This task is additive — it creates new files and adds new CSS without removing the topbar, so the app still works with the topbar until we wire it up in Task 2.

**Files:**
- Create: `crates/koji-web/src/components/sidebar.rs`
- Modify: `crates/koji-web/style.css`
- Modify: `crates/koji-web/src/components/mod.rs`

**What to implement:**

1. **CSS custom properties** (add to `:root` in `style.css`, after existing custom properties around line 33):
   ```css
   --sidebar-width-expanded: 200px;
   --sidebar-width-collapsed: 56px;
   ```

2. **Sidebar CSS classes** (add to `style.css` in the Layout section, immediately after the existing `.topbar` rules. Do NOT remove `.topbar` CSS yet — that happens in Task 2.):
   
   ```css
   /* Sidebar Layout */
   .sidebar {
       width: var(--sidebar-width-expanded);
       height: 100vh;
       position: fixed;
       top: 0;
       left: 0;
       z-index: 100;
       background-color: var(--bg-secondary);
       border-right: 1px solid var(--border-color);
       display: flex;
       flex-direction: column;
        transition: width var(--transition-normal); /* Desktop collapse/expand only; mobile uses transform */
       overflow: hidden;
   }
   
   .sidebar--collapsed {
       width: var(--sidebar-width-collapsed);
   }
   
   .sidebar-header {
       padding: 1rem;
       display: flex;
       align-items: center;
       gap: 0.5rem;
       text-decoration: none;
       color: var(--text-primary);
       font-weight: 700;
       font-size: 1.1rem;
       border-bottom: 1px solid var(--border-color);
       min-height: 52px;
   }
   
   .sidebar-header:hover {
       background-color: var(--bg-tertiary);
       text-decoration: none;
   }
   
   .sidebar-header__logo {
       flex-shrink: 0;
       font-size: 1.2rem;
   }
   
   .sidebar-header__text {
       white-space: nowrap;
       overflow: hidden;
   }
   
   .sidebar--collapsed .sidebar-header__text {
       display: none;
   }
   
   .sidebar-nav {
       flex: 1;
       padding: 0.5rem;
       display: flex;
       flex-direction: column;
       gap: 0.125rem;
       overflow-y: auto;
   }
   
   .sidebar-section {
       margin-top: 0.5rem;
       padding-top: 0.5rem;
       border-top: 1px solid var(--border-color);
   }
   
   .sidebar-item {
       display: flex;
       align-items: center;
       gap: 0.75rem;
       padding: 0.5rem 0.75rem;
       border-radius: var(--radius-sm);
       color: var(--text-secondary);
       text-decoration: none;
       transition: background var(--transition-fast), color var(--transition-fast);
       white-space: nowrap;
       overflow: hidden;
   }
   
   .sidebar-item:hover {
       color: var(--text-primary);
       background-color: var(--bg-tertiary);
       text-decoration: none;
   }
   
   .sidebar-item[aria-current="page"] {
       color: var(--accent-blue);
       background-color: var(--bg-tertiary);
   }
   
   .sidebar-item__icon {
       flex-shrink: 0;
       font-size: 1.1rem;
       line-height: 1;
       width: 1.5rem;
       text-align: center;
   }
   
   .sidebar-item__text {
       font-weight: 500;
       overflow: hidden;
   }
   
   .sidebar--collapsed .sidebar-item__text {
       display: none;
   }
   
   /* Tooltip for collapsed sidebar items */
   .sidebar--collapsed .sidebar-item {
       position: relative;
   }
   
   .sidebar--collapsed .sidebar-item:hover::after {
       content: attr(data-tooltip);
       position: absolute;
       left: 100%;
       top: 50%;
       transform: translateY(-50%);
       margin-left: 0.5rem;
       padding: 0.25rem 0.5rem;
       background-color: var(--bg-tertiary);
       color: var(--text-primary);
       border: 1px solid var(--border-color);
       border-radius: var(--radius-sm);
       font-size: 0.85rem;
       white-space: nowrap;
       z-index: 200;
       pointer-events: none;
   }
   
   .sidebar-footer {
       padding: 0.5rem;
       border-top: 1px solid var(--border-color);
       display: flex;
       flex-direction: column;
       gap: 0.125rem;
   }
   
   .sidebar-toggle {
       display: flex;
       align-items: center;
       justify-content: center;
       gap: 0.5rem;
       padding: 0.5rem;
       border-radius: var(--radius-sm);
       color: var(--text-muted);
       background: none;
       border: 1px solid transparent;
       cursor: pointer;
       font-size: 0.85rem;
       transition: background var(--transition-fast), color var(--transition-fast);
       width: 100%;
   }
   
   .sidebar-toggle:hover {
       color: var(--text-primary);
       background-color: var(--bg-tertiary);
   }
   
   .sidebar-toggle__icon {
       flex-shrink: 0;
       transition: transform var(--transition-normal);
   }
   
   .sidebar--collapsed .sidebar-toggle__icon {
       transform: rotate(180deg);
   }
   
   .sidebar-toggle__text {
       overflow: hidden;
   }
   
   .sidebar--collapsed .sidebar-toggle__text {
       display: none;
   }
   ```

3. **Sidebar component** — Create `crates/koji-web/src/components/sidebar.rs`:
   
   ```rust
   use leptos::prelude::*;
   use leptos_router::components::A;
   
   #[component]
   pub fn Sidebar() -> impl IntoView {
       let collapsed = RwSignal::new(false);
       
       // On mount, read localStorage for persisted state
       // Use wasm_bindgen to access window.localStorage
       // Key: "koji-sidebar-collapsed"
       // If value is "true", set collapsed to true
       
       // Effect to persist collapsed state to localStorage
       
       view! {
           <aside class="sidebar" class:sidebar--collapsed=move || collapsed.get()>
               // Header: logo link to "/"
               <A href="/" attr:class="sidebar-header">
                   <span class="sidebar-header__logo">"⚡"</span>
                   <span class="sidebar-header__text">"Koji"</span>
               </A>
               
               // Main nav items
               <nav class="sidebar-nav">
                   <A href="/" attr:class="sidebar-item" attr:data-tooltip="Dashboard">
                       <span class="sidebar-item__icon">"🏠"</span>
                       <span class="sidebar-item__text">"Dashboard"</span>
                   </A>
                   <A href="/models" attr:class="sidebar-item" attr:data-tooltip="Models">
                       <span class="sidebar-item__icon">"📦"</span>
                       <span class="sidebar-item__text">"Models"</span>
                   </A>
                   <A href="/backends" attr:class="sidebar-item" attr:data-tooltip="Backends">
                       <span class="sidebar-item__icon">"🔧"</span>
                       <span class="sidebar-item__text">"Backends"</span>
                   </A>
                   <A href="/logs" attr:class="sidebar-item" attr:data-tooltip="Logs">
                       <span class="sidebar-item__icon">"📋"</span>
                       <span class="sidebar-item__text">"Logs"</span>
                   </A>
               </nav>
               
               // Footer: Config (separated) + toggle
               <div class="sidebar-footer">
                   <div class="sidebar-section" style="border-top:none;margin:0;padding:0;">
                       <A href="/config" attr:class="sidebar-item" attr:data-tooltip="Config">
                           <span class="sidebar-item__icon">"⚙️"</span>
                           <span class="sidebar-item__text">"Config"</span>
                       </A>
                   </div>
                   <button class="sidebar-toggle" on:click=move |_| collapsed.update(|c| *c = !*c)>
                       <span class="sidebar-toggle__icon">"↔"</span>
                       <span class="sidebar-toggle__text">"Collapse"</span>
                   </button>
               </div>
           </aside>
       }
   }
   ```
   
   For the localStorage interop, use this pattern. Note: the codebase consistently uses `web_sys::window()` (not `leptos::prelude::window()`), so follow that convention. You also need to add `"Storage"` to the `web-sys` features in `crates/koji-web/Cargo.toml` (see step 0 below).
   ```rust
   use web_sys::window;
   
   // On mount, read localStorage — use a plain closure, NOT Effect::new,
   // since this has no reactive dependencies (runs once only)
   let initial = window()
       .and_then(|w| w.local_storage().ok())
       .flatten()
       .and_then(|ls| ls.get("koji-sidebar-collapsed").ok())
       .flatten();
   if initial.as_deref() == Some("true") {
       collapsed.set(true);
   }
   
   // Persist state when it changes — this IS reactive (subscribes to collapsed)
   Effect::new(move || {
       let val = if collapsed.get() { "true" } else { "false" };
       if let (Some(w), Some(ls)) = (
           window(),
           window().and_then(|w| w.local_storage().ok()).flatten(),
       ) {
           let _ = ls.set("koji-sidebar-collapsed", val);
       }
   });
   ```
   Note: `web_sys::window()` returns `Option<Window>`. `.local_storage()` returns `Result<Option<Storage>, JsValue>`. Handle with `.ok()`, `.flatten()`, and `.and_then()` chains.

0. **Add `Storage` feature to `web-sys`** — Before writing the localStorage code, add `"Storage"` to the `web-sys` features list in `crates/koji-web/Cargo.toml`. The current features are: `Window`, `Document`, `HtmlElement`, `HtmlInputElement`, `HtmlSelectElement`, `HtmlTextAreaElement`, `EventSource`, `EventSourceInit`, `MessageEvent`, `Event`, `KeyboardEvent`, `MouseEvent`, `SvgElement`, `SvgsvgElement`, `DomRect`, `console`. Add `"Storage"` to this list.

5. **Register module** — In `crates/koji-web/src/components/mod.rs`, add `pub mod sidebar;` at the end.

**Steps:**
- [ ] Add `"Storage"` feature to `web-sys` features in `crates/koji-web/Cargo.toml`
- [ ] Add CSS custom properties and sidebar CSS classes to `crates/koji-web/style.css`
- [ ] Create `crates/koji-web/src/components/sidebar.rs` with the `Sidebar` component
- [ ] Add `pub mod sidebar;` to `crates/koji-web/src/components/mod.rs`
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat(ui): add sidebar component and CSS classes"

**Acceptance criteria:**
- [ ] `Sidebar` component compiles without errors
- [ ] All sidebar CSS classes are defined in `style.css`
- [ ] CSS custom properties `--sidebar-width-expanded` and `--sidebar-width-collapsed` exist in `:root`
- [ ] App still renders with the old topbar (no visual change yet)

---

### Task 2: Wire Sidebar Into App Layout + Remove Topbar

**Context:**
The sidebar component and CSS are ready from Task 1. Now we need to swap the topbar for the sidebar in the app's root layout, remove the topbar CSS, and delete the old `nav.rs` component. After this task, the app will render with the left sidebar instead of the top bar.

**Files:**
- Modify: `crates/koji-web/src/lib.rs`
- Modify: `crates/koji-web/style.css`
- Delete: `crates/koji-web/src/components/nav.rs`
- Modify: `crates/koji-web/src/components/mod.rs`

**What to implement:**

1. **Update `lib.rs`** — Change the `App` component view:
   
   **Before:**
   ```rust
   view! {
       <Router>
           <components::nav::Nav />
           <main>
               <Routes fallback=|| "Page not found">
                   ...
               </Routes>
           </main>
       </Router>
   }
   ```
   
   **After:**
   ```rust
   view! {
       <Router>
           <components::sidebar::Sidebar />
           <main>
               <Routes fallback=|| "Page not found">
                   ...
               </Routes>
           </main>
       </Router>
   }
   ```
   
   Note: The sidebar uses `position: fixed`, so `<main>` naturally flows beside it with `margin-left`.

2. **Update `main` CSS** — In `style.css`, change the `main` rule:
   
   **Before (lines 91–95):**
   ```css
   main {
       max-width: 1200px;
       margin: 0 auto;
       padding: 2rem 1.5rem;
   }
   ```
   
   **After:**
   ```css
   main {
       margin-left: var(--sidebar-width-expanded);
       padding: 2rem 1.5rem;
       min-height: 100vh;
       transition: margin-left var(--transition-normal);
   }
   ```
   
   Remove `max-width: 1200px` and `margin: 0 auto`. Pages that need a max-width can set it themselves (or we keep it for pages that don't have inner sidebars — but that's a per-page concern). The `transition` on `margin-left` animates when the sidebar collapses/expands.

3. **Remove `.topbar` CSS** — Delete all `.topbar` related rules from `style.css` (lines 59–95 approximately, the entire `.topbar` block including `.topbar .logo`, `.topbar a`, `.topbar a:hover`, `.topbar a[aria-current="page"]`). Keep the `/* 3. Layout */` comment header.

4. **Delete `nav.rs`** — Remove the file `crates/koji-web/src/components/nav.rs`.

5. **Update `mod.rs`** — Remove `pub mod nav;` from `crates/koji-web/src/components/mod.rs`.

**Steps:**
- [ ] Update `lib.rs` to use `<components::sidebar::Sidebar />` instead of `<components::nav::Nav />`
- [ ] Update `main` CSS in `style.css` to use `margin-left` and remove `max-width`
- [ ] Remove all `.topbar` CSS rules from `style.css`
- [ ] Remove `pub mod nav;` from `components/mod.rs`
- [ ] Delete `crates/koji-web/src/components/nav.rs`
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix compilation errors (likely missing import references) and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, investigate and fix.
- [ ] Commit with message: "feat(ui): replace topbar with collapsible left sidebar"

**Acceptance criteria:**
- [ ] App renders with a left sidebar showing all 5 nav items (Dashboard, Models, Backends, Logs, Config)
- [ ] Config link is at the bottom, separated by a divider
- [ ] Sidebar collapses to icons when toggle button is clicked
- [ ] Sidebar expands back when toggle is clicked again
- [ ] No topbar rendered anywhere
- [ ] Active page is highlighted in sidebar
- [ ] `nav.rs` file is deleted
- [ ] Visual smoke test: open the app in a browser and verify the sidebar renders correctly with all nav items before committing

---

### Task 3: Fix Page-Level Sidebar Sticky Offsets

**Context:**
The Config Editor and Model Editor pages have inner section-navigation sidebars that previously used `position: sticky; top: 1rem` to stay visible while scrolling. This `1rem` offset was needed to avoid the 52px sticky topbar. Now that the topbar is gone, these sidebars should use `top: 0` so they stick flush to the top of the content area. Without this fix, the page sidebars will have a visual gap at the top when scrolled.

**Files:**
- Modify: `crates/koji-web/src/pages/config_editor.rs`
- Modify: `crates/koji-web/style.css`

**What to implement:**

1. **Config Editor sidebar** — In `crates/koji-web/src/pages/config_editor.rs`, find the inline `<nav>` element (around line 245) with:
   ```rust
   <nav class="card" style="width:220px;flex-shrink:0;padding:0.75rem;position:sticky;top:1rem;">
   ```
   Change `top:1rem` to `top:0`:
   ```rust
   <nav class="card" style="width:220px;flex-shrink:0;padding:0.75rem;position:sticky;top:0;">
   ```

2. **Model Editor sidebar** — In `crates/koji-web/style.css`, find the `.model-editor-nav` rule (around line 792):
   ```css
   .model-editor-nav {
       width: 200px;
       flex-shrink: 0;
       display: flex;
       flex-direction: column;
       gap: 0.25rem;
       position: sticky;
       top: 1rem;
   }
   ```
   Change `top: 1rem` to `top: 0`:
   ```css
   .model-editor-nav {
       width: 200px;
       flex-shrink: 0;
       display: flex;
       flex-direction: column;
       gap: 0.25rem;
       position: sticky;
       top: 0;
   }
   ```

**Steps:**
- [ ] Fix `top: 1rem` → `top: 0` in `config_editor.rs` inline style
- [ ] Fix `top: 1rem` → `top: 0` in `.model-editor-nav` CSS rule
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "fix(ui): correct page sidebar sticky offsets after topbar removal"

**Acceptance criteria:**
- [ ] Config Editor section nav sticks flush to top when scrolling (no gap)
- [ ] Model Editor section nav sticks flush to top when scrolling (no gap)
- [ ] Neither sidebar slides under any overlay element

---

### Task 4: Delete Dead Code (config_nav.rs)

**Context:**
`crates/koji-web/src/components/config_nav.rs` contains an unused `ConfigNav` component marked with `#[allow(dead_code)]`. The Config Editor page (`config_editor.rs`) has its own inline sidebar implementation, making this component dead code. Now that we've confirmed the new sidebar works, we should clean up this unused file.

**Files:**
- Delete: `crates/koji-web/src/components/config_nav.rs`
- Modify: `crates/koji-web/src/components/mod.rs`

**What to implement:**

1. Delete `crates/koji-web/src/components/config_nav.rs`.
2. Remove `pub mod config_nav;` from `crates/koji-web/src/components/mod.rs`.

**Steps:**
- [ ] Remove `pub mod config_nav;` from `components/mod.rs`
- [ ] Delete `crates/koji-web/src/components/config_nav.rs`
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, there may be other files importing `config_nav` — find and remove those imports.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "chore(ui): remove unused ConfigNav component"

**Acceptance criteria:**
- [ ] `config_nav.rs` file is deleted
- [ ] No references to `config_nav` remain in the codebase
- [ ] `config_editor.rs` has its own section definitions and doesn't reference `ConfigSection` (verified — it uses its own inline `Section` enum)
- [ ] `cargo check --workspace` passes

---

### Task 5: Add Mobile Responsive Overlay

**Context:**
On narrow screens (<768px), the sidebar should not take up permanent space. Instead, it should be hidden by default and appear as an overlay when a hamburger button is tapped. This is the standard responsive pattern for admin sidebars. The hamburger button lives in a slim top bar that only appears on mobile.

**Files:**
- Modify: `crates/koji-web/src/components/sidebar.rs`
- Modify: `crates/koji-web/style.css`

**What to implement:**

1. **Mobile toggle button + overlay logic in `sidebar.rs`:**
   
   Add a `mobile_open: RwSignal<bool>` signal. When on mobile, clicking the hamburger toggles this signal. Clicking the overlay backdrop closes it. **Important:** When a user taps a sidebar nav link on mobile, the overlay must auto-close. Add an `on:click` handler on each `<A>` that sets `mobile_open.set(false)` (even though `<A>` navigates, the overlay should close immediately on click, not wait for route change).
   
   ```rust
   #[component]
   pub fn Sidebar() -> impl IntoView {
       let collapsed = RwSignal::new(false);
       let mobile_open = RwSignal::new(false);
       
       // ... localStorage persistence for collapsed (same as Task 1) ...
       
       view! {
           // Mobile hamburger toggle (hidden on desktop)
           <button class="sidebar-mobile-toggle" on:click=move |_| mobile_open.set(true)>
               "☰"
           </button>
           
           // Overlay backdrop (hidden when mobile_open is false)
           <div
               class="sidebar-overlay"
               class:sidebar-overlay--visible=move || mobile_open.get()
               on:click=move |_| mobile_open.set(false)
           />
           
           <aside
               class="sidebar"
               class:sidebar--collapsed=move || collapsed.get()
               class:sidebar--mobile-open=move || mobile_open.get()
           >
               // Close button for mobile (top-right, hidden on desktop)
               <button
                   class="sidebar-close"
                   on:click=move |_| mobile_open.set(false)
               >
                   "✕"
               </button>
               
               // Header: logo link — close mobile overlay on click
               <A href="/" attr:class="sidebar-header" on:click=move |_| mobile_open.set(false)>
                   <span class="sidebar-header__logo">"⚡"</span>
                   <span class="sidebar-header__text">"Koji"</span>
               </A>
               
               // Main nav items — each closes mobile overlay on click
               <nav class="sidebar-nav">
                   <A href="/" attr:class="sidebar-item" attr:data-tooltip="Dashboard" on:click=move |_| mobile_open.set(false)>
                       <span class="sidebar-item__icon">"🏠"</span>
                       <span class="sidebar-item__text">"Dashboard"</span>
                   </A>
                   // ... same on:click pattern for Models, Backends, Logs, Config ...
               </nav>
               
               // ... footer with toggle (same as Task 1) ...
           </aside>
       }
   }
   ```

2. **Mobile CSS in `style.css`:**
   
   Z-index hierarchy (documented here for clarity):
   - Desktop sidebar: `z-index: 100` (always visible, fixed left)
   - Mobile hamburger toggle: `z-index: 998` (must be above overlay)
   - Mobile overlay backdrop: `z-index: 999`
   - Mobile sidebar: `z-index: 1000` (must be above overlay)
   
   ```css
   /* Mobile toggle — hidden on desktop */
   .sidebar-mobile-toggle {
       display: none;
       position: fixed;
       top: 0;
       left: 0;
       z-index: 998;
       width: 52px;
       height: 52px;
       background-color: var(--bg-secondary);
       border: none;
       border-bottom: 1px solid var(--border-color);
       border-right: 1px solid var(--border-color);
       color: var(--text-primary);
       font-size: 1.25rem;
       cursor: pointer;
       align-items: center;
       justify-content: center;
   }
   
   .sidebar-close {
       display: none;
       position: absolute;
       top: 0.75rem;
       right: 0.75rem;
       background: none;
       border: none;
       color: var(--text-muted);
       font-size: 1.25rem;
       cursor: pointer;
   }
   
   .sidebar-close:hover {
       color: var(--text-primary);
   }
   
   /* Overlay backdrop */
   .sidebar-overlay {
       display: none;
       position: fixed;
       top: 0;
       left: 0;
       width: 100%;
       height: 100%;
       background-color: rgba(0, 0, 0, 0.5);
       z-index: 999;
   }
   
   .sidebar-overlay--visible {
       display: block;
   }
   
   @media (max-width: 768px) {
       .sidebar {
           transform: translateX(-100%);
           z-index: 1000;
           /* Width transition is for desktop collapse/expand; mobile uses transform */
       }
       
       .sidebar--mobile-open {
           transform: translateX(0);
       }
       
       .sidebar-mobile-toggle {
           display: flex;
       }
       
       .sidebar-close {
           display: block;
       }
       
       main {
           margin-left: 0;
           padding-top: 52px; /* space for the hamburger bar */
       }
   }
   ```

**Steps:**
- [ ] Add `mobile_open: RwSignal<bool>` signal and hamburger/close button to `sidebar.rs`
- [ ] Add overlay backdrop `<div>` to `sidebar.rs`
- [ ] Add all mobile CSS classes and `@media` query to `style.css`
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Test in browser at narrow width: hamburger appears, sidebar slides in, close/backdrop dismisses
- [ ] Commit with message: "feat(ui): add mobile responsive sidebar overlay"

**Acceptance criteria:**
- [ ] On desktop (>768px), sidebar renders normally (fixed left, no hamburger)
- [ ] On mobile (<768px), sidebar is hidden by default
- [ ] Hamburger button appears in top-left on mobile
- [ ] Clicking hamburger slides sidebar in from left as overlay
- [ ] Dark backdrop appears behind overlay sidebar
- [ ] Clicking backdrop or ✕ closes the overlay
- [ ] `main` content has no left margin on mobile

---

### Task Summary

| Task | Description | Files Changed |
|------|-------------|---------------|
| 1 | Create sidebar CSS + component | `sidebar.rs` (new), `style.css`, `mod.rs` |
| 2 | Wire sidebar in, remove topbar | `lib.rs`, `style.css`, delete `nav.rs` |
| 3 | Fix page sidebar sticky offsets | `config_editor.rs`, `style.css` |
| 4 | Delete dead code | delete `config_nav.rs`, `mod.rs` |
| 5 | Mobile responsive overlay | `sidebar.rs`, `style.css` |
