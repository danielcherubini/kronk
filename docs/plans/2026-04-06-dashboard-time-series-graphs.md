# Dashboard Time-Series Graphs Plan

**Goal:** Replace the current static gauge/number cards on the Dashboard with live SVG sparkline/area charts that show system metrics (CPU, RAM, GPU, VRAM) over the last 5 minutes.

**Architecture:** The existing 3-second polling loop on the dashboard already fetches `SystemHealth` from `/koji/v1/system/health`. We will accumulate these snapshots into a client-side ring buffer (max 100 entries ≈ 5 minutes at 3s intervals). A new reusable `<SparklineChart>` Leptos component will render the buffered data as responsive SVG area charts. No backend changes required — all history is kept in browser memory.

**Tech Stack:** Leptos 0.7 (CSR), SVG rendering in Leptos `view!` macros, `RwSignal<Vec<SystemHealth>>` as the ring buffer.

---

### Task 1: Create the SparklineChart component

**Context:**
The dashboard needs a reusable charting component to render time-series data as SVG area charts. This component will be used for CPU%, RAM usage, GPU%, and VRAM usage. It takes a slice of `f32` values (normalized 0–100 or raw values) and renders an SVG with a filled area and a line stroke, colored according to the dashboard's existing dark theme CSS variables. The component should be self-contained, responsive (fills its container width), and have no external dependencies.

**Files:**
- Create: `crates/koji-web/src/components/sparkline.rs`
- Modify: `crates/koji-web/src/components/mod.rs` — add `pub mod sparkline;`

**What to implement:**

Create a `SparklineChart` Leptos component with these props:
- `data: Vec<f32>` — the values to plot (most recent last). This is a plain `Vec<f32>`, NOT a signal. The component is re-created on each render cycle inside a reactive closure, so plain props work correctly here.
- `max_value: f32` — the maximum Y-axis value (e.g., 100.0 for percentages, or `ram_total_mib` for memory)
- `color: String` — CSS color for the line/fill (e.g. `"var(--accent-green)"`)
- `height: f32` — SVG height in px (default 60.0)

The SVG should:
- Be `width="100%"` and use a **fixed** `viewBox` of `"0 0 100 {height}"` so the chart always fills its container consistently regardless of data count.
- X coordinates should be scaled: `x = (i as f32 / (data.len() - 1).max(1) as f32) * 100.0`. This ensures the chart fills the full width whether there are 2 data points or 100.
- Draw **two** `<path>` elements:
  1. A **fill path** (the area under the line): `stroke="none"`, `fill={color}`, `fill-opacity="0.15"`. This path starts at the first data point's Y position, traces through all points, then drops to the bottom-right and bottom-left to close.
  2. A **line path** (the line itself): `fill="none"`, `stroke={color}`, `stroke-width="1.5"`. This path traces through all data points without closing to the bottom.
- Handle edge cases:
  - **Empty data**: render an empty `<svg>` with just the viewBox (no paths).
  - **Single data point**: duplicate it (treat as two points at x=0 and x=100) to draw a flat horizontal line at the value's Y position.
- The `<svg>` element should have `class="sparkline"` and `preserveAspectRatio="none"` so it stretches to fill its container.

**Path construction for the fill path:**
```
M 0,{y0} L {x1},{y1} L {x2},{y2} ... L {xN},{yN} L 100,{height} L 0,{height} Z
```
Where `y = height - (value / max_value * height)`, clamped to `[0, height]`.

**Path construction for the line path:**
```
M 0,{y0} L {x1},{y1} L {x2},{y2} ... L {xN},{yN}
```
(Same points, no closure to the bottom.)

Also in `components/mod.rs`, add `pub mod sparkline;` alongside the existing `pub mod nav;`.

**Steps:**
- [ ] Create `crates/koji-web/src/components/sparkline.rs` with the `SparklineChart` component as described above
- [ ] Add `pub mod sparkline;` to `crates/koji-web/src/components/mod.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? The WASM target may require `--target wasm32-unknown-unknown` for the koji-web crate. If clippy fails on koji-web specifically, try `cargo clippy --package koji-core --package koji-cli --package koji-mock -- -D warnings` to at least validate the non-WASM crates, and visually verify the koji-web code compiles with `cargo build --package koji-web --target wasm32-unknown-unknown` (if the target is installed) or just `cargo check --package koji-web`.
- [ ] Commit with message: `feat: add SparklineChart SVG component for dashboard graphs`

**Acceptance criteria:**
- [ ] `SparklineChart` component exists and compiles
- [ ] Component handles empty data (no panic, renders empty SVG)
- [ ] Component handles single data point (renders flat horizontal line)
- [ ] SVG uses fixed `viewBox="0 0 100 {height}"` for consistent scaling
- [ ] Two separate paths: one for the fill area (15% opacity) and one for the stroke line
- [ ] Fill path does NOT have a visible stroke along the bottom edge
- [ ] No new external dependencies added

---

### Task 2: Add CSS styles for the sparkline charts and updated card layout

**Context:**
The existing dashboard cards use `.grid-stats` with `.card`, `.card-header`, `.card-value`, and `.gauge` / `.gauge-fill` classes. We need to update the layout so each card can show both the current value AND a sparkline chart below it. The existing gauge bars will be replaced by the sparkline charts. We also need CSS for the new `.sparkline` SVG element.

**Files:**
- Modify: `crates/koji-web/style.css`

**What to implement:**

Add the following CSS rules to `style.css` (append after the existing gauge section, around line 313):

```css
/* 11b. Sparkline charts */
.sparkline {
    display: block;
    width: 100%;
    height: 60px;
    margin-top: 0.75rem;
}

.sparkline-container {
    position: relative;
    width: 100%;
}
```

Also modify the `.grid-stats` rule to use a slightly wider minimum column width so the sparklines have room to breathe. Change `minmax(200px, 1fr)` to `minmax(240px, 1fr)`.

Do NOT remove or modify the existing `.gauge` / `.gauge-fill` CSS rules — they may be used elsewhere or in the future. Just add the new sparkline rules.

**Steps:**
- [ ] Add the sparkline CSS rules to `crates/koji-web/style.css` after the gauge section (after line ~313)
- [ ] Update `.grid-stats` `grid-template-columns` from `minmax(200px, 1fr)` to `minmax(240px, 1fr)`
- [ ] Run `cargo fmt --all` (CSS won't be affected but good practice)
- [ ] Commit with message: `feat: add sparkline chart CSS styles for dashboard`

**Acceptance criteria:**
- [ ] `.sparkline` class is defined with `width: 100%` and `height: 60px`
- [ ] `.grid-stats` has updated minimum column width
- [ ] Existing gauge CSS is preserved (not deleted)

---

### Task 3: Refactor the Dashboard to use a metrics history ring buffer and render sparkline charts

**Context:**
This is the main task. The current `Dashboard` component polls `/koji/v1/system/health` every 3 seconds and displays the latest snapshot as gauge bars and numbers. We need to:
1. Accumulate each health snapshot into a `Vec<SystemHealth>` ring buffer (max 100 entries, ~5 minutes)
2. Replace the gauge bars with `SparklineChart` components showing the time-series for each metric
3. Keep the current value display (the big number) above each chart
4. Refactor BOTH the grid view AND the page header to read from the `history` signal (not from the `health` resource directly) to avoid double-consumption of the resource value

The existing `SystemHealth` struct is defined locally in `dashboard.rs`. It already has all the fields we need: `cpu_usage_pct`, `ram_used_mib`, `ram_total_mib`, `gpu_utilization_pct`, and `vram` (with `used_mib`/`total_mib`).

**Files:**
- Modify: `crates/koji-web/src/pages/dashboard.rs`

**What to implement:**

1. **Add the import** at the top of dashboard.rs:
   ```rust
   use crate::components::sparkline::SparklineChart;
   ```

2. **Add a history signal** at the top of the `Dashboard` component function body (after the existing `refresh` signal):
   ```rust
   let history = RwSignal::new(Vec::<SystemHealth>::new());
   ```

3. **Add a fetch-failure tracking signal** to preserve the error/retry state:
   ```rust
   let fetch_failed = RwSignal::new(false);
   ```

4. **Add an Effect to accumulate snapshots.** Keep the existing `refresh` signal, `setInterval`, and `LocalResource` (`health`) exactly as they are. Add an `Effect::new` that watches the resource and appends to `history`:
   ```rust
   Effect::new(move |_| {
       if let Some(guard) = health.get() {
           if let Some(h) = (*guard).clone() {
               fetch_failed.set(false);
               history.update(|buf| {
                   buf.push(h);
                   if buf.len() > 100 {
                       buf.drain(..buf.len() - 100);
                   }
               });
           } else {
               fetch_failed.set(true);
           }
       }
   });
   ```

   **CRITICAL NOTES:**
   - Use `move |_|` (with one parameter), NOT `move ||`. Leptos 0.7's `Effect::new` passes the previous return value to the closure.
   - Use `(*guard).clone()` to clone the inner `Option<SystemHealth>`, NOT `guard.take()`. The `take()` method consumes the value, which would cause the old view code to see `None`. By cloning, we don't interfere with any other subscribers. (Though after step 5 below, there won't be other subscribers reading the resource directly — this is still safer.)

5. **Refactor the page header** (currently lines 86–100 in dashboard.rs) to read from `history` instead of from the `health` resource. Replace:
   ```rust
   // OLD — reads from `health` resource (REMOVE this)
   <Suspense>
       {move || {
           health.get().map(|guard| {
               let h = guard.take();
               h.map(|h| view! { ... })
           })
       }}
   </Suspense>
   ```
   With:
   ```rust
   // NEW — reads from `history` signal
   {move || {
       history.get().last().cloned().map(|h| view! {
           <div class="flex-between gap-1">
               <span class={format!("badge {}", status_badge_class(&h.status))}>{h.status.clone()}</span>
               <button class="btn btn-secondary btn-sm" on:click=move |_| { restart.dispatch(()); }>"Restart"</button>
           </div>
       })
   }}
   ```
   Note: No `<Suspense>` needed here since we're reading from an `RwSignal`, not a resource.

6. **Refactor the main grid view.** Remove the outer `<Suspense>` that wraps the grid (currently lines 102–167). Replace it with a direct reactive closure reading from `history`. The `<Suspense>` component only tracks `Resource`/`LocalResource`, not `RwSignal`, so it won't work correctly with the new approach.

   The new structure:
   ```rust
   {move || {
       let buf = history.get();
       if fetch_failed.get() && buf.is_empty() {
           // Network error, no data yet — show error with retry button
           return view! {
               <div class="card">
                   <p class="text-error">"Failed to load health data. Is Koji running?"</p>
                   <button class="btn btn-secondary btn-sm mt-2" on:click=manual_refresh>"Retry"</button>
               </div>
           }.into_any();
       }
       match buf.last().cloned() {
           Some(h) => view! {
               <div class="grid-stats">
                   // CPU card
                   <div class="card">
                       <div class="card-header">"CPU Usage"</div>
                       <div class="card-value">{format!("{:.1}%", h.cpu_usage_pct)}</div>
                       <SparklineChart
                           data=buf.iter().map(|s| s.cpu_usage_pct).collect::<Vec<f32>>()
                           max_value=100.0
                           color="var(--accent-green)".to_string()
                           height=60.0
                       />
                   </div>

                   // Memory card
                   <div class="card">
                       <div class="card-header">"Memory"</div>
                       <div class="card-value">{format!("{} / {} MiB", h.ram_used_mib, h.ram_total_mib)}</div>
                       <SparklineChart
                           data=buf.iter().map(|s| s.ram_used_mib as f32).collect::<Vec<f32>>()
                           max_value={h.ram_total_mib as f32}
                           color="var(--accent-blue)".to_string()
                           height=60.0
                       />
                   </div>

                   // GPU card — only rendered if GPU data is present in the latest snapshot.
                   // For the data Vec, use .map() with unwrap_or(0) instead of .filter_map()
                   // to keep time-axis aligned with other charts.
                   {h.gpu_utilization_pct.map(|pct| view! {
                       <div class="card">
                           <div class="card-header">"GPU"</div>
                           <div class="card-value">{format!("{}%", pct)}</div>
                           <SparklineChart
                               data=buf.iter().map(|s| s.gpu_utilization_pct.unwrap_or(0) as f32).collect::<Vec<f32>>()
                               max_value=100.0
                               color="var(--accent-yellow)".to_string()
                               height=60.0
                           />
                       </div>
                   })}

                   // VRAM card — only rendered if VRAM data is present in the latest snapshot.
                   {h.vram.as_ref().map(|v| {
                       let total = v.total_mib as f32;
                       view! {
                           <div class="card">
                               <div class="card-header">"VRAM"</div>
                               <div class="card-value">{format!("{} / {} MiB", v.used_mib, v.total_mib)}</div>
                               <SparklineChart
                                   data=buf.iter().map(|s| s.vram.as_ref().map(|v| v.used_mib as f32).unwrap_or(0.0)).collect::<Vec<f32>>()
                                   max_value=total
                                   color="var(--accent-purple)".to_string()
                                   height=60.0
                               />
                           </div>
                       }
                   })}

                   // Models Loaded — keep as simple number, no chart
                   <div class="card">
                       <div class="card-header">"Models Loaded"</div>
                       <div class="card-value">{h.models_loaded}</div>
                   </div>
               </div>
           }.into_any(),
           None => view! {
               <div class="card card--centered">
                   <span class="spinner">"Loading dashboard..."</span>
               </div>
           }.into_any(),
       }
   }}
   ```

7. **Remove all gauge elements.** Delete every `<div class="gauge">` and `<div class="gauge-fill" .../>` from the dashboard view.

8. **Clean up unused helpers.** After the refactor, check if `color_for_pct` is still used anywhere. It was previously used for gauge bar colors. If it's no longer referenced (the sparkline charts use their own fixed color per metric), remove it. Keep `status_badge_class` — it's still used for the page header badge.

**Leptos prop syntax note:** In Leptos 0.7 `view!` macros, component props use `prop_name=expression` syntax WITHOUT curly braces around the value. For example: `data=buf.iter().map(...).collect::<Vec<f32>>()`, NOT `data={buf.iter()...}`. String literals use `color="var(--accent-green)".to_string()`.

**Steps:**
- [ ] Add `use crate::components::sparkline::SparklineChart;` to dashboard.rs imports
- [ ] Add the `history` signal and `fetch_failed` signal
- [ ] Add the `Effect::new(move |_| { ... })` block to accumulate health snapshots (use `(*guard).clone()`, NOT `guard.take()`)
- [ ] Refactor the page header to read from `history.get().last()` instead of from the `health` resource directly — remove the `<Suspense>` wrapper around the header badge
- [ ] Remove the outer `<Suspense>` around the grid and replace with the reactive `move || { ... }` closure reading from `history`
- [ ] Implement the full grid view with `SparklineChart` components for CPU, Memory, GPU, VRAM
- [ ] For GPU/VRAM data vectors, use `.map()` with `.unwrap_or(0)` defaults (NOT `.filter_map()`) to keep time-axis aligned
- [ ] Remove all `<div class="gauge">` and `<div class="gauge-fill" .../>` elements
- [ ] Remove `color_for_pct` helper if it's no longer used; keep `status_badge_class`
- [ ] Keep the "Models Loaded" card as a simple number (no chart)
- [ ] Preserve the error/retry state — if fetch fails and history is empty, show the error message with retry button
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings` (or check koji-web specifically)
- [ ] Visually verify by building with Trunk if possible: `trunk serve` in `crates/koji-web/`
- [ ] Commit with message: `feat: replace dashboard gauges with time-series sparkline charts`

**Acceptance criteria:**
- [ ] Dashboard shows sparkline area charts for CPU, RAM, GPU (if available), and VRAM (if available)
- [ ] Each chart shows up to 100 data points (~5 minutes of history)
- [ ] Current value is still displayed as a large number above each chart
- [ ] History accumulates over time as new polls arrive every 3 seconds
- [ ] Page reload starts with empty history (expected behavior for client-side buffer)
- [ ] "Models Loaded" card remains as a simple numeric display
- [ ] Status badge in page header reads from `history` signal (not `health` resource)
- [ ] Error/retry state is preserved — shows error message when fetch fails and no history exists
- [ ] GPU and VRAM data vectors use `.unwrap_or(0)` defaults, keeping time-axis aligned
- [ ] No `<Suspense>` wrapping the grid (since it reads from `RwSignal`, not `Resource`)
- [ ] No new external dependencies added
- [ ] Empty state (before first poll) shows a loading spinner
