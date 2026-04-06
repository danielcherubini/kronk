# Web Control Plane UI Redesign

**Goal:** Transform the Koji web control plane from unstyled browser-default HTML into a sleek, modern dark dashboard with live-updating metrics, VRAM breakdowns, and polished visual design.

**Architecture:** A single hand-crafted CSS file (`style.css`) added to the Trunk build pipeline provides all styling via CSS custom properties (variables) for easy theming. The existing Leptos component structure stays intact — we add CSS classes to existing `view!` macros and enhance the Dashboard page with auto-refreshing metrics, visual gauges, and per-model VRAM bars. No npm, no external build tools — just CSS linked from `index.html` and processed by Trunk.

**Tech Stack:** Leptos 0.7 (CSR/WASM), Trunk asset pipeline, hand-crafted CSS with custom properties, CSS Grid/Flexbox for layout, CSS animations for transitions.

**Design Direction:** Modern dark dashboard (Grafana/Portainer-inspired). Dark background (`#0d1117`), card-based layout with subtle borders, blue/cyan accent colors, monospace for data values, smooth transitions.

**Prerequisites / Assumptions:**
- Leptos 0.7 with CSR feature — `on_cleanup` requires `Send + Sync + 'static` closures (WASM types like `gloo_timers::Interval` are NOT `Send`, so we cannot use `on_cleanup(move || drop(interval))`)
- Trunk processes `<link data-trunk rel="css" href="...">` tags to copy CSS into `dist/`
- `include_dir!("$CARGO_MANIFEST_DIR/dist")` in `server.rs` embeds the entire `dist/` directory at compile time
- The existing `<main>` wrapper in `lib.rs` provides the container element for page layout
- `gloo-timers` is already in `Cargo.toml` — no new dependencies needed
- The `<A>` component from `leptos_router` passes through standard HTML attributes like `class` — if this doesn't work, fall back to wrapping `<a>` tags with manual navigation

---

### Task 1: Create CSS Design System & Link to Trunk Build

**Context:**
Currently the web UI has zero CSS — not a single stylesheet, no classes (except two unused ones), and all output uses browser defaults. This task creates the foundational CSS file with the design system (colors, typography, spacing, component styles) and wires it into the Trunk build pipeline so it gets embedded in the `dist/` output. This is the foundation everything else builds on.

Trunk automatically processes `<link data-trunk ...>` tags in `index.html` to copy/bundle assets into `dist/`. We add a `<link data-trunk rel="css" href="style.css">` to `index.html` and create the CSS file at `crates/koji-web/style.css`.

**Files:**
- Create: `crates/koji-web/style.css`
- Modify: `crates/koji-web/index.html`

**What to implement:**

Create `style.css` with the following sections:

1. **CSS Custom Properties** (`:root`):
   - `--bg-primary: #0d1117` (main background)
   - `--bg-secondary: #161b22` (card/panel background)
   - `--bg-tertiary: #21262d` (elevated surfaces, table rows)
   - `--border-color: #30363d` (subtle borders)
   - `--border-hover: #484f58` (hover state borders)
   - `--text-primary: #e6edf3` (main text)
   - `--text-secondary: #8b949e` (muted text, labels)
   - `--text-muted: #6e7681` (very subtle text)
   - `--accent-blue: #58a6ff` (links, primary actions)
   - `--accent-green: #3fb950` (success, loaded, healthy)
   - `--accent-red: #f85149` (errors, failures)
   - `--accent-yellow: #d29922` (warnings, disabled)
   - `--accent-purple: #bc8cff` (info, special)
   - `--accent-cyan: #39d2c0` (metrics, gauges)
   - `--font-body: -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans', Helvetica, Arial, sans-serif`
   - `--font-mono: 'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace`
   - `--radius-sm: 4px`
   - `--radius-md: 8px`
   - `--radius-lg: 12px`
   - `--shadow-card: 0 1px 3px rgba(0,0,0,0.3), 0 1px 2px rgba(0,0,0,0.4)`
   - `--transition-fast: 150ms ease`
   - `--transition-normal: 250ms ease`

2. **Reset & Base styles**:
   - `*, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }`
   - `body` — bg `--bg-primary`, color `--text-primary`, font `--font-body`, line-height 1.6
   - `a` — color `--accent-blue`, no underline, hover underline
   - `code` — font `--font-mono`, bg `--bg-tertiary`, padding 0.15em 0.4em, border-radius `--radius-sm`

3. **Layout**:
   - `nav.topbar` — bg `--bg-secondary`, border-bottom `1px solid var(--border-color)`, height 52px, flex row, align center, padding 0 1.5rem, gap 0.5rem, position sticky, top 0, z-index 100
   - `nav.topbar .logo` — font-weight 700, font-size 1.1rem, color `--text-primary`, margin-right 2rem
   - `nav.topbar a` — color `--text-secondary`, padding 0.5rem 0.75rem, border-radius `--radius-sm`, transition `--transition-fast`. Hover: color `--text-primary`, bg `--bg-tertiary`
   - `nav.topbar a[aria-current="page"]` — color `--accent-blue`, bg `--bg-tertiary` (Leptos router sets this attribute on the active link)
   - `main` — max-width 1200px, margin 0 auto, padding 2rem 1.5rem (note: `lib.rs` already wraps `<Routes>` in a `<main>` tag)

4. **Cards** (`.card`):
   - bg `--bg-secondary`, border `1px solid var(--border-color)`, border-radius `--radius-lg`, padding 1.5rem, box-shadow `--shadow-card`
   - `.card-header` — font-size 0.85rem, text-transform uppercase, letter-spacing 0.05em, color `--text-secondary`, margin-bottom 1rem
   - `.card-value` — font-size 2rem, font-weight 700, font-family `--font-mono`, color `--text-primary`

5. **Grid layouts**:
   - `.grid-stats` — display grid, grid-template-columns `repeat(auto-fit, minmax(200px, 1fr))`, gap 1rem
   - `.grid-2col` — display grid, grid-template-columns `1fr 1fr`, gap 1.5rem
   - `.form-grid` — display grid, grid-template-columns `180px 1fr`, gap 0.75rem 1rem, align-items center
   - `@media (max-width: 768px)` — `.grid-2col, .grid-stats { grid-template-columns: 1fr; }` and `.form-grid { grid-template-columns: 1fr; }`

6. **Tables** (`.data-table`):
   - width 100%, border-collapse separate, border-spacing 0
   - `th` — bg `--bg-tertiary`, color `--text-secondary`, text-transform uppercase, font-size 0.75rem, letter-spacing 0.05em, padding 0.75rem 1rem, text-align left, border-bottom `2px solid var(--border-color)`
   - `td` — padding 0.75rem 1rem, border-bottom `1px solid var(--border-color)`, font-family `--font-mono` for data cells
   - `tr:hover td` — bg `rgba(56, 139, 253, 0.05)`
   - `tr:last-child td` — border-bottom none

7. **Buttons**:
   - `.btn` — display inline-flex, align center, padding 0.5rem 1rem, border-radius `--radius-md`, font-size 0.875rem, font-weight 500, cursor pointer, transition `--transition-fast`, border none
   - `.btn-primary` — bg `--accent-blue`, color white. Hover: filter brightness(1.1)
   - `.btn-secondary` — bg `--bg-tertiary`, color `--text-primary`, border `1px solid var(--border-color)`. Hover: border-color `--border-hover`
   - `.btn-danger` — bg `--accent-red`, color white. Hover: filter brightness(1.1)
   - `.btn-success` — bg `--accent-green`, color `--bg-primary`
   - `.btn:disabled` — opacity 0.4, cursor not-allowed
   - `.btn-sm` — padding 0.3rem 0.6rem, font-size 0.8rem

8. **Forms**:
   - `input[type="text"], input[type="number"], select, textarea` — bg `--bg-tertiary`, border `1px solid var(--border-color)`, color `--text-primary`, padding 0.5rem 0.75rem, border-radius `--radius-sm`, font-family `--font-mono` (for text/number/textarea), font-size 0.9rem, transition `--transition-fast`, width 100%
   - Focus: border-color `--accent-blue`, outline none, box-shadow `0 0 0 2px rgba(88,166,255,0.2)`
   - `label` — color `--text-secondary`, font-size 0.85rem, font-weight 500, display block, margin-bottom 0.25rem

9. **Progress bars** (`.progress-bar`):
   - Container: bg `--bg-tertiary`, height 8px, border-radius 4px, overflow hidden
   - Fill: `.progress-bar-fill` — height 100%, bg `linear-gradient(90deg, var(--accent-blue), var(--accent-cyan))`, border-radius 4px, transition width 0.3s ease
   - Indeterminate: `.progress-bar-fill.indeterminate` — width 30%, animation `indeterminate 1.5s ease infinite`
   - `@keyframes indeterminate { 0% { margin-left: 0; } 50% { margin-left: 70%; } 100% { margin-left: 0; } }`

10. **Status badges** (`.badge`):
    - display inline-flex, padding 0.15rem 0.6rem, border-radius 999px, font-size 0.75rem, font-weight 600
    - `.badge-success` — bg `rgba(63,185,80,0.15)`, color `--accent-green`
    - `.badge-error` — bg `rgba(248,81,73,0.15)`, color `--accent-red`
    - `.badge-warning` — bg `rgba(210,153,34,0.15)`, color `--accent-yellow`
    - `.badge-info` — bg `rgba(88,166,255,0.15)`, color `--accent-blue`
    - `.badge-muted` — bg `--bg-tertiary`, color `--text-muted`

11. **Gauges** (`.gauge`):
    - `.gauge` — position relative, width 100%, height 6px, bg `--bg-tertiary`, border-radius 3px, overflow hidden
    - `.gauge-fill` — position absolute, top 0, left 0, height 100%, border-radius 3px, transition width 0.5s ease
    - `.gauge-label` — display flex, justify-content space-between, font-size 0.8rem, color `--text-secondary`, margin-bottom 0.25rem

12. **Utility classes**:
    - `.text-mono` — font-family `--font-mono`
    - `.text-muted` — color `--text-secondary`
    - `.text-success` — color `--accent-green`
    - `.text-error` — color `--accent-red`
    - `.text-warning` — color `--accent-yellow`
    - `.mt-1` through `.mt-4` — margin-top 0.5rem/1rem/1.5rem/2rem
    - `.mb-1` through `.mb-4` — margin-bottom
    - `.flex-between` — display flex, justify-content space-between, align-items center
    - `.gap-1` — gap 0.5rem; `.gap-2` — gap 1rem

13. **Page header** (`.page-header`):
    - display flex, justify-content space-between, align-items center, margin-bottom 1.5rem
    - `h1` — font-size 1.5rem, font-weight 600

14. **Log viewer** (`.log-viewer`):
    - bg `--bg-tertiary`, color `--text-primary`, font-family `--font-mono`, font-size 0.8rem, padding 1rem, border-radius `--radius-md`, border `1px solid var(--border-color)`, max-height 600px, overflow-y auto, white-space pre-wrap, line-height 1.5

15. **Spinner** (`.spinner`):
    - `@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }`
    - `.spinner::before` — content "", display inline-block, width 1rem, height 1rem, border `2px solid var(--border-color)`, border-top-color `--accent-blue`, border-radius 50%, animation spin 0.6s linear infinite, margin-right 0.5rem, vertical-align middle

16. **Wizard steps** (`.wizard`):
    - `.wizard-steps` — display flex, gap 0.5rem, margin-bottom 2rem
    - `.wizard-step` — flex 1, padding 0.5rem, text-align center, font-size 0.8rem, color `--text-muted`, border-bottom `2px solid var(--border-color)`
    - `.wizard-step.active` — color `--accent-blue`, border-bottom-color `--accent-blue`
    - `.wizard-step.completed` — color `--accent-green`, border-bottom-color `--accent-green`

Modify `index.html` to add the CSS link and a viewport meta tag:
```html
<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Koji Control Plane</title>
    <link data-trunk rel="css" href="style.css" />
  </head>
  <body></body>
</html>
```

**Steps:**
- [ ] Create `crates/koji-web/style.css` with all sections described above
- [ ] Modify `crates/koji-web/index.html` to add viewport meta and CSS link with `data-trunk` attribute
- [ ] Run `cd crates/koji-web && trunk build` to verify Trunk picks up the CSS and includes it in `dist/`
  - Did the build succeed and does `dist/` contain the CSS (either inlined or as a separate file)?
- [ ] Run `cargo build --workspace` to verify the SSR build still compiles (the `include_dir!` macro embeds the new `dist/`)
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add CSS design system for web control plane dark theme"

**Acceptance criteria:**
- [ ] `style.css` exists at `crates/koji-web/style.css` with all 16 sections
- [ ] `index.html` includes `<link data-trunk rel="css" href="style.css" />`
- [ ] `trunk build` succeeds and CSS appears in `dist/` output
- [ ] `cargo build --workspace` succeeds (SSR embeds the new dist)
- [ ] CSS custom properties define a cohesive dark theme color palette

---

### Task 2: Restyle Navigation Bar

**Context:**
The navigation component (`crates/koji-web/src/components/nav.rs`) currently renders pipe-separated plain text links: `Dashboard | Models | Pull Model | Logs | Config`. This task applies the `topbar` CSS classes from the design system (created in Task 1) to transform it into a professional sticky top navigation bar with the Koji branding. This is a visual-only change — no logic changes.

**Files:**
- Modify: `crates/koji-web/src/components/nav.rs`

**What to implement:**

Replace the entire `view!` block in the `Nav` component. The new markup should be:

```rust
view! {
    <nav class="topbar">
        <span class="logo">"⚡ Koji"</span>
        <A href="/" class="nav-link">"Dashboard"</A>
        <A href="/models" class="nav-link">"Models"</A>
        <A href="/pull" class="nav-link">"Pull Model"</A>
        <A href="/logs" class="nav-link">"Logs"</A>
        <A href="/config" class="nav-link">"Config"</A>
    </nav>
}
```

Remove all `" | "` text separators. Add `class="topbar"` to `<nav>`. Add `class="logo"` to the brand span. Add `class="nav-link"` to each `<A>` tag.

**Note on `<A>` and `class`:** The Leptos router `<A>` component renders as a standard `<a>` element and should pass through `class` as an HTML attribute. If compilation or rendering fails with `class` on `<A>`, fall back to: `<a href="/" class="nav-link">"Dashboard"</a>` — which loses Leptos's client-side routing but still works. Test this early in this task.

Leptos router adds `aria-current="page"` on the active `<A>` link. The CSS rule `nav.topbar a[aria-current="page"] { color: var(--accent-blue); background: var(--bg-tertiary); }` should already be in `style.css` from Task 1 (section 3, Layout) — verify it's there, and add it if not.

**Steps:**
- [ ] Modify `crates/koji-web/src/components/nav.rs` with the new markup as described above
- [ ] If needed, add the `a[aria-current="page"]` CSS rule to `style.css`
- [ ] Run `cd crates/koji-web && trunk build`
  - Did it succeed?
- [ ] Run `cargo build --workspace`
  - Did it succeed?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did it succeed?
- [ ] Commit with message: "feat: restyle web nav bar with dark theme topbar"

**Acceptance criteria:**
- [ ] Nav renders as a horizontal bar with logo and styled links
- [ ] No pipe separators remain
- [ ] Active nav link is visually distinct
- [ ] `trunk build` and `cargo build --workspace` succeed

---

### Task 3: Redesign Dashboard with Metric Cards, Gauges, and Auto-Refresh

**Context:**
The Dashboard page (`crates/koji-web/src/pages/dashboard.rs`) currently shows plain `<p>` tags with system metrics and a plain button. This task is the biggest visual transformation — it redesigns the Dashboard into a card-based layout with:
- Status header with service state badge
- Grid of metric cards (CPU, RAM, GPU, VRAM, models loaded)
- Visual gauge bars for CPU/RAM/GPU/VRAM utilization
- Auto-refresh every 3 seconds using `gloo_timers`
- Per-model VRAM breakdown (if the API provides per-model data)
- Uptime display

The existing `SystemHealth` struct and API call stay the same. We wrap the data in styled markup using CSS classes from Task 1.

**Files:**
- Modify: `crates/koji-web/src/pages/dashboard.rs`

**What to implement:**

1. **Add auto-refresh**: Create a `refresh` signal (`RwSignal<u32>`) and use `gloo_timers::callback::Interval` to increment it every 3 seconds. The `LocalResource` should track this signal to re-fetch.

   **IMPORTANT**: `gloo_timers::callback::Interval` is NOT `Send`, so it CANNOT be moved into Leptos's `on_cleanup()` (which requires `Send + Sync + 'static`). Instead, use `std::mem::forget` to intentionally leak the interval (it lives for the lifetime of the page anyway), OR store it as a `thread_local!`. The simplest correct pattern:
   ```rust
   let refresh = RwSignal::new(0u32);
   // Leak the interval — it runs for the page's lifetime.
   // Leptos CSR components are not unmounted/remounted like SSR, so this is safe.
   std::mem::forget(gloo_timers::callback::Interval::new(3_000, move || {
       refresh.update(|n| *n += 1);
   }));
   ```
   If the component IS remounted (e.g., route changes), the interval would leak. A more robust alternative is to use `wasm_bindgen::closure::Closure` with `web_sys::window().set_interval_with_callback_and_timeout_and_arguments_0()` and store the interval ID in a signal, clearing it with `clear_interval` on route change. However, since the Dashboard is a top-level page component that only exists while navigated to, the simple `forget` approach is acceptable.

   Update the `LocalResource::new` closure to read `refresh.get()` so it re-fires.

2. **Restructure the view** to use this layout:
   ```rust
   <div class="page-header">
       <h1>"Dashboard"</h1>
       <div class="flex-between gap-1">
           <span class=format!("badge {}", status_badge_class(&h.status))>{&h.status}</span>
           <button class="btn btn-secondary btn-sm" on:click=move |_| { restart.dispatch(()); }>"Restart"</button>
       </div>
   </div>

   <div class="grid-stats">
       <!-- CPU Card -->
       <div class="card">
           <div class="card-header">"CPU Usage"</div>
           <div class="card-value">{cpu_pct}"%"</div>
           <div class="gauge">
               <div class="gauge-fill" style=format!("width:{}%; background:{}", cpu_pct, color_for_pct(cpu_pct)) />
           </div>
       </div>

       <!-- RAM Card -->
       <div class="card">
           <div class="card-header">"Memory"</div>
           <div class="card-value">{ram_used}" / "{ram_total}" MiB"</div>
           <div class="gauge">
               <div class="gauge-fill" style=format!("width:{}%; background:var(--accent-blue)", ram_pct) />
           </div>
       </div>

       <!-- GPU Card (conditional) -->
       <!-- VRAM Card (conditional) -->
       <!-- Models Loaded Card -->
       <div class="card">
           <div class="card-header">"Models Loaded"</div>
           <div class="card-value">{models_loaded}</div>
       </div>
   </div>
   ```

3. **Helper function** `color_for_pct(pct: f32) -> &'static str`:
   - `< 60.0` → `"var(--accent-green)"`
   - `< 85.0` → `"var(--accent-yellow)"`
   - `>= 85.0` → `"var(--accent-red)"`
   - Note: `gpu_utilization_pct` is `Option<u8>`, so cast with `pct as f32` when calling this function

4. **Status badge class**: Map the `status` string:
   - `"ok"` → `"badge-success"`
   - `"degraded"` → `"badge-warning"`
   - anything else → `"badge-error"`

5. **VRAM gauge**: When `vram` is Some, render a card with:
   - Header: "VRAM"
   - Value: `"{used} / {total} MiB"`
   - Gauge fill: width as percentage `(used as f32 / total as f32) * 100.0`, color based on `color_for_pct`

6. **GPU utilization gauge**: When `gpu_utilization_pct` is Some, render a card with:
   - Header: "GPU"
   - Value: `"{pct}%"`
   - Gauge fill: width = pct, color based on `color_for_pct`

7. **Loading state**: The `Suspense` fallback should show a spinner:
   ```
   <div class="card" style="text-align:center; padding:3rem;">
       <span class="spinner">"Loading dashboard..."</span>
   </div>
   ```

8. **Error state**: When health data is `None`, show:
   ```
   <div class="card">
       <p class="text-error">"Failed to load health data. Is Koji running?"</p>
       <button class="btn btn-secondary btn-sm" on:click=manual_refresh>"Retry"</button>
   </div>
   ```

**Steps:**
- [ ] Modify `crates/koji-web/src/pages/dashboard.rs` as described above
- [ ] Add `use gloo_timers::callback::Interval;` import
- [ ] Add the `color_for_pct` helper function
- [ ] Run `cd crates/koji-web && trunk build`
  - Did it succeed?
- [ ] Run `cargo build --workspace`
  - Did it succeed?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did it succeed?
- [ ] Commit with message: "feat: redesign dashboard with metric cards, gauges, and auto-refresh"

**Acceptance criteria:**
- [ ] Dashboard shows metric cards in a responsive grid
- [ ] Each metric has a colored gauge bar
- [ ] Auto-refresh updates every 3 seconds
- [ ] Status badge shows green/yellow/red based on health
- [ ] GPU/VRAM cards only appear when data is available
- [ ] Loading and error states are styled
- [ ] `trunk build` and `cargo build --workspace` succeed

---

### Task 4: Restyle Models List Page

**Context:**
The Models page (`crates/koji-web/src/pages/models.rs`) currently renders a plain HTML table with default browser styling and unstyled buttons. This task applies the design system's `.data-table`, `.btn`, `.badge`, and `.page-header` classes to make it match the dark dashboard theme.

**Files:**
- Modify: `crates/koji-web/src/pages/models.rs`

**What to implement:**

1. **Page header** — Replace `<h1>"Models"</h1>` and the button div with:
   ```
   <div class="page-header">
       <h1>"Models"</h1>
       <A href="/models/new/edit">
           <button class="btn btn-primary">"+ New Model"</button>
       </A>
   </div>
   ```

2. **Table** — Add `class="data-table"` to `<table>`:
   ```
   <table class="data-table">
   ```

3. **Enabled column** — Replace plain "Yes"/"No" text with badges:
   - If `m.enabled`: `<span class="badge badge-success">"Enabled"</span>`
   - Else: `<span class="badge badge-warning">"Disabled"</span>`

4. **Loaded column** — Replace "Loaded"/"Unloaded" text with badges:
   - If `m.loaded`: `<span class="badge badge-success">"Loaded"</span>`
   - Else: `<span class="badge badge-muted">"Idle"</span>`

5. **Action buttons** — Add button classes:
   - Unload button: `class="btn btn-secondary btn-sm"`
   - Load button: `class="btn btn-success btn-sm"`
   - Edit button: `class="btn btn-secondary btn-sm"`

6. **Loading state** — Replace `<p>"Loading..."</p>` with:
   ```
   <div style="text-align:center; padding:2rem;">
       <span class="spinner">"Loading models..."</span>
   </div>
   ```

7. **Error state** — Replace `<p>"Failed to load models"</p>` with:
   ```
   <div class="card">
       <p class="text-error">"Failed to load models. Is Koji running?"</p>
   </div>
   ```

8. **Empty state** — After the table, if `data.models` is empty, show:
   ```
   <div class="card" style="text-align:center; padding:2rem;">
       <p class="text-muted">"No models configured yet."</p>
       <A href="/pull"><button class="btn btn-primary mt-2">"Pull a Model"</button></A>
   </div>
   ```

**Steps:**
- [ ] Modify `crates/koji-web/src/pages/models.rs` with all changes described above
- [ ] Run `cd crates/koji-web && trunk build`
  - Did it succeed?
- [ ] Run `cargo build --workspace`
  - Did it succeed?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did it succeed?
- [ ] Commit with message: "feat: restyle models list with dark theme table and badges"

**Acceptance criteria:**
- [ ] Models table uses `.data-table` styling
- [ ] Enabled/Loaded columns show colored badges instead of plain text
- [ ] Action buttons are styled with appropriate colors
- [ ] Loading, error, and empty states are styled
- [ ] `trunk build` and `cargo build --workspace` succeed

---

### Task 5: Restyle Pull Wizard

**Context:**
The Pull wizard (`crates/koji-web/src/pages/pull.rs`) is a multi-step flow (RepoInput → Loading → SelectQuants → SetContext → Downloading → Done) that currently uses plain HTML with no visual distinction between steps and unstyled form controls. This task adds a step indicator bar at the top, styled cards around each step's content, styled buttons, and custom progress bars to replace the native `<progress>` elements.

**Files:**
- Modify: `crates/koji-web/src/pages/pull.rs`

**What to implement:**

1. **Step indicator**: Add a step indicator bar at the top of the wizard, **outside** the main `move || match wizard_step.get()` block, so it renders reactively based on the wizard step signal. Create a helper function `fn step_class(step_num: u8, current: u8) -> &'static str` that returns `"active"` if step_num == current, `"completed"` if step_num < current, or `""` otherwise. Map `WizardStep` to number: `RepoInput=1, LoadingQuants=2, SelectQuants=3, SetContext=4, Downloading=5, Done=6`. The step indicator uses reactive closures:
   ```rust
   <div class="wizard-steps">
       {move || {
           let current = wizard_step_to_num(wizard_step.get());
           view! {
               <div class=format!("wizard-step {}", step_class(1, current))>"1. Repo"</div>
       <div class=format!("wizard-step {}", step_class(2, current))>"2. Loading"</div>
       <div class=format!("wizard-step {}", step_class(3, current))>"3. Select"</div>
       <div class=format!("wizard-step {}", step_class(4, current))>"4. Context"</div>
               <div class=format!("wizard-step {}", step_class(5, current))>"5. Download"</div>
               <div class=format!("wizard-step {}", step_class(6, current))>"6. Done"</div>
           }
       }}
   </div>
   ```
   This pattern ensures the step indicator re-renders reactively when `wizard_step` changes, since the closure tracks the signal.

2. **Wrap each step's content in a `.card`**: Each step body should be wrapped in `<div class="card">...</div>`.

3. **Step 1 (RepoInput)**: 
   - `<h1>` → use page-header pattern
   - Input field: no `<label>` inline text — use `<label>` element with proper class
   - Search button: `class="btn btn-primary"`
   - Error: `<p class="text-error">` instead of inline `style="color:red"`

4. **Step 2 (Loading)**: Show `.spinner` class on the message.

5. **Step 3 (SelectQuants)**: 
   - Table: `class="data-table"`
   - Select All / Deselect All: `class="btn btn-secondary btn-sm"`
   - Back: `class="btn btn-secondary"`, Next: `class="btn btn-primary"`
   - Size column: add `class="text-mono"`

6. **Step 4 (SetContext)**:
   - Table: `class="data-table"`
   - Number inputs should pick up the form styling from CSS automatically
   - Back: `class="btn btn-secondary"`, Start Download: `class="btn btn-primary"`

7. **Step 5 (Downloading)**:
   - Replace native `<progress>` elements with custom progress bars:
     ```
     <div class="progress-bar">
         <div class="progress-bar-fill" style=format!("width:{}%", pct) />
     </div>
     ```
     Where `pct = if total > 0 { (downloaded as f64 / total as f64) * 100.0 } else { 0.0 }`. For indeterminate progress (no total), use a CSS animation by adding a class like `"progress-bar-fill indeterminate"` and a CSS rule for it (add to `style.css`: `.progress-bar-fill.indeterminate { width: 30%; animation: indeterminate 1.5s ease infinite; }` with `@keyframes indeterminate { 0% { margin-left: 0; } 50% { margin-left: 70%; } 100% { margin-left: 0; } }`).
   - Status text: use badges — completed: `.badge-success`, failed: `.badge-error`, running: no badge (just text)
   - Filename: `<strong>` with `.text-mono`
   - Error lists: `class="text-error"`

8. **Step 6 (Done)**:
   - Success message with checkmark
   - "View Models" link: `class="btn btn-primary"`

9. **Remove all inline `style=` attributes** from the Pull component — everything should use CSS classes, **except** dynamic computed values (e.g., progress bar `style=format!("width:{}%", pct)` which must remain inline).

**Steps:**
- [ ] Modify `crates/koji-web/src/pages/pull.rs` with all changes described above
- [ ] Add the `indeterminate` progress bar animation to `style.css` if not already present
- [ ] Run `cd crates/koji-web && trunk build`
  - Did it succeed?
- [ ] Run `cargo build --workspace`
  - Did it succeed?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did it succeed?
- [ ] Commit with message: "feat: restyle pull wizard with step indicator and styled progress bars"

**Acceptance criteria:**
- [ ] Step indicator shows current/completed/pending steps
- [ ] All buttons use `.btn` classes
- [ ] Tables use `.data-table` class
- [ ] Progress bars use custom styled divs instead of native `<progress>`
- [ ] No inline `style=` attributes remain in pull.rs except dynamic computed values (progress bar widths)
- [ ] `trunk build` and `cargo build --workspace` succeed

---

### Task 6: Restyle Model Editor

**Context:**
The Model Editor (`crates/koji-web/src/pages/model_editor.rs`, ~690 lines) is the largest and most complex page in the web UI. It has two forms (model config + model card), a dynamic quants sub-table, and heavy use of inline styles. This task converts it from `<table>`-based form layout to CSS Grid and applies the full design system. It's split as its own task due to complexity.

The model editor has two `<table>`-based forms and a quants sub-table rendered with a `<For>` component. The form tables should be converted to `.form-grid` CSS Grid layout. The quants sub-table should remain a `<table>` but use the `.data-table` class.

**Files:**
- Modify: `crates/koji-web/src/pages/model_editor.rs`

**What to implement:**

1. Replace the `<table>`-based form layout with semantic form markup using CSS Grid. Instead of `<table><tr><td>Label</td><td>Input</td></tr>...</table>`, use:
   ```
   <div class="form-grid">
       <label>"Backend"</label>
       <select ...>...</select>
       
       <label>"Model"</label>
       <input type="text" ... />
       ...
   </div>
   ```
   The `.form-grid` CSS class (added in Task 1) provides the two-column grid layout.

2. Page header: Use `.page-header` with the model name and a delete button (`.btn-danger`).

3. Wrap Model Config and Model Card sections in separate `.card` divs with `.card-header` titles:
   - Card 1: "Model Configuration" header, containing the form fields for backend, model, quant, profile, context, port, enabled, args
   - Card 2: "Model Card" header, containing the card metadata fields (name, source, default context, default GPU layers) and the quants sub-table

4. Quants sub-table: Add `class="data-table"` to the existing `<table>` that renders quants. Keep the `<For>` component structure but add appropriate classes to cells.

5. Save buttons: `class="btn btn-primary"`. Delete: `class="btn btn-danger"`.

6. Status messages: Replace inline `style="color:green"` / `style="color:red"` with `class="text-success"` / `class="text-error"`.

7. "Add quant" button: `class="btn btn-secondary btn-sm"`. Remove quant button: `class="btn btn-danger btn-sm"`.

8. Remove ALL inline `style=` attributes — use CSS classes instead. Exception: dynamic computed values that must remain inline.

**Steps:**
- [ ] Modify `crates/koji-web/src/pages/model_editor.rs` with all changes described
- [ ] Run `cd crates/koji-web && trunk build`
  - Did it succeed?
- [ ] Run `cargo build --workspace`
  - Did it succeed?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did it succeed?
- [ ] Commit with message: "feat: restyle model editor with dark theme form grid and cards"

**Acceptance criteria:**
- [ ] Model editor forms use CSS Grid layout (`.form-grid`) instead of table
- [ ] Both form sections wrapped in `.card` divs
- [ ] Quants sub-table uses `.data-table` class
- [ ] No inline `style=` attributes remain (except dynamic computed values)
- [ ] All buttons use `.btn` classes
- [ ] Status messages use `.text-success` / `.text-error`
- [ ] `trunk build` and `cargo build --workspace` succeed

---

### Task 7: Restyle Logs and Config Editor

**Context:**
The Logs page (`logs.rs`) and Config Editor page (`config_editor.rs`) are the two simplest pages — Logs is a pre-formatted text viewer with a refresh button, and Config Editor is a textarea with save/reload. Both are quick wins to apply the dark theme styling to.

**Files:**
- Modify: `crates/koji-web/src/pages/logs.rs`
- Modify: `crates/koji-web/src/pages/config_editor.rs`

**What to implement:**

**Logs (`logs.rs`):**

1. Page header: Use `.page-header` with "Logs" title and Refresh button (`class="btn btn-secondary btn-sm"`).

2. Replace the `<pre>` with inline styles with `<pre class="log-viewer">` — the CSS already has the `.log-viewer` styles from Task 1 (section 14) with dark background, monospace font, max-height, and overflow.

3. Loading: Use `.spinner` class. Error: Use `.text-error` class.

4. Remove all inline `style=` attributes.

**Config Editor (`config_editor.rs`):**

1. Page header: Use `.page-header` with "Config" title and action buttons (Save: `class="btn btn-primary"`, Reload: `class="btn btn-secondary"`).

2. Wrap the textarea in a `.card` div.

3. The `<textarea>` will automatically pick up form styling from CSS (the Task 1 CSS includes `textarea` in the form element selector with monospace font, dark background, and borders).

4. Status messages: `class="text-success"` / `class="text-error"` instead of inline styles.

5. Remove all inline `style=` attributes.

**Steps:**
- [ ] Modify `crates/koji-web/src/pages/logs.rs` with all changes described
- [ ] Modify `crates/koji-web/src/pages/config_editor.rs` with all changes described
- [ ] Run `cd crates/koji-web && trunk build`
  - Did it succeed?
- [ ] Run `cargo build --workspace`
  - Did it succeed?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did it succeed?
- [ ] Commit with message: "feat: restyle logs and config editor with dark theme"

**Acceptance criteria:**
- [ ] Logs page uses `.log-viewer` class on the pre element
- [ ] Config editor textarea is wrapped in a `.card`
- [ ] No inline `style=` attributes remain in either file
- [ ] All buttons use `.btn` classes
- [ ] Status messages use `.text-success` / `.text-error`
- [ ] `trunk build` and `cargo build --workspace` succeed

---

### Task 8: Rebuild Trunk Dist and Verify Full Build

**Context:**
After all the visual changes, the `dist/` directory (which is committed to the repo and embedded at compile time via `include_dir!`) needs to be rebuilt with the final version of the CSS and WASM. This task runs the full build pipeline, verifies everything works, and ensures the committed `dist/` is up to date.

**Files:**
- Modify: `crates/koji-web/dist/*` (rebuilt by Trunk)

**What to implement:**

1. Run `trunk build --release` in `crates/koji-web/` to produce optimized WASM + JS + CSS output in `dist/`.

2. Run the full workspace build: `cargo build --workspace` — this compiles the SSR server which embeds `dist/` via `include_dir!`.

3. Run `cargo test --workspace` to ensure nothing is broken.

4. Run `cargo clippy --workspace -- -D warnings` for lint checks.

5. Run `cargo fmt --all` for formatting.

**Steps:**
- [ ] Run `cd crates/koji-web && trunk build --release`
  - Did it succeed? Check that `dist/` contains the CSS file (or it's inlined into the HTML).
- [ ] Run `cargo build --workspace`
  - Did it succeed?
- [ ] Run `cargo test --workspace`
  - Did all tests pass?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did it succeed?
- [ ] Commit with message: "chore: rebuild web dist with styled control plane"

**Acceptance criteria:**
- [ ] `dist/` contains up-to-date built assets (WASM, JS, CSS)
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --all` produces no changes
