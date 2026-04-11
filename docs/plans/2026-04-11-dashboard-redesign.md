# Dashboard Metrics Redesign Plan

**Goal:** Redesign the dashboard stat cards to fix alignment issues, add sparkline interactivity (hover tooltips, Y-axis refs, time labels), add a history API endpoint, and load historical data on page mount.

**Architecture:** Backend adds a REST endpoint that reads from the existing `system_metrics_history` SQLite table. The dashboard frontend fetches history on mount, then joins the SSE stream. Cards use flex layout with fixed height. SparklineChart gains hover interactivity via Leptos reactive signals.

**Tech Stack:** Leptos 0.7 (Rust → WASM, CSR), Axum 0.7 (HTTP server), rusqlite (SQLite), SVG for sparklines, hand-crafted CSS.

---

### Task 1: Add history API endpoint (`GET /koji/v1/system/metrics/history`)

**Context:** The frontend currently only receives metrics via SSE, which starts empty. Metrics are already persisted in `system_metrics_history` (SQLite) by the background metrics task. Two query functions exist (`get_recent_system_metrics`, `get_system_metrics_since`) but are not wired to any HTTP route. This task creates the endpoint so the dashboard can load historical data on page mount.

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers/system.rs`
- Modify: `crates/koji-core/src/proxy/koji_handlers/mod.rs`
- Modify: `crates/koji-core/src/proxy/server/router.rs`

**What to implement:**

1. In `system.rs`, add a `MetricsHistoryEntry` response struct:
   ```rust
   #[derive(Debug, Serialize)]
   pub struct MetricsHistoryEntry {
       pub ts_unix_ms: i64,
       pub cpu_usage_pct: f32,
       pub ram_used_mib: i64,
       pub ram_total_mib: i64,
       pub gpu_utilization_pct: Option<i64>,
       pub vram_used_mib: Option<i64>,
       pub vram_total_mib: Option<i64>,
   }
   ```
   Note: Use `i64` fields for memory/GPU to match `SystemMetricsRow`. The frontend will convert.

2. In `system.rs`, add `handle_system_metrics_history` handler:
   - Extract `limit` from query string (default 100, max 1000, parse with `axum::extract::Query`)
   - Call `state.open_db()` — if `None`, return `Json(vec![])` (HTTP 200, not an error)
   - If `Some(conn)`, call `get_recent_system_metrics(&conn, limit)` from `crate::db::queries::metrics_queries`
   - Map each `SystemMetricsRow` → `MetricsHistoryEntry`
   - Return `Json(Vec<MetricsHistoryEntry>)`

3. In `koji_handlers/mod.rs`, add export:
   ```rust
   pub use system::{handle_system_metrics_history, /* existing exports */};
   ```

4. In `server/router.rs`, add route:
   ```rust
   .route("/koji/v1/system/metrics/history", get(handle_system_metrics_history))
   ```
   Import `handle_system_metrics_history` in the use statement.

5. Add `axum::extract::Query` import in `system.rs` and a `HistoryQueryParams` struct:
   ```rust
   #[derive(Debug, serde::Deserialize)]
   pub struct HistoryQueryParams {
       #[serde(default = "default_limit")]
       pub limit: i64,
   }
   fn default_limit() -> i64 { 100 }
   ```
   Clamp `limit` to 1..=1000 range in the handler.

**Steps:**
- [ ] Run `cargo test --package koji-core` — all existing tests pass
- [ ] Add `MetricsHistoryEntry`, `HistoryQueryParams`, and `handle_system_metrics_history` to `system.rs`
- [ ] Add import for `crate::db::queries::metrics_queries` and `axum::extract::Query` in `system.rs`
- [ ] Add `handle_system_metrics_history` to `mod.rs` exports
- [ ] Add route and import in `router.rs`
- [ ] Run `cargo test --package koji-core`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: add GET /koji/v1/system/metrics/history endpoint"

**Acceptance criteria:**
- [ ] Endpoint returns JSON array of `MetricsHistoryEntry` objects
- [ ] Returns empty array (HTTP 200) when DB is unavailable
- [ ] Returns empty array when no rows exist
- [ ] `limit` query param defaults to 100, clamped to 1–1000
- [ ] All existing tests still pass
- [ ] Server compiles and route is registered

---

### Task 2: Add interactive hover support to SparklineChart

**Context:** The sparkline is currently a pure SVG with no interactivity. When hovering over the chart, users should see: a vertical indicator line, a highlighted dot on the data line, and a tooltip with the exact value and relative timestamp. On mouse leave, overlays disappear. The chart also needs boundary reference lines (0% and 100% for percentages, max capacity for memory) and time axis labels ("now" at right, "-Xm" at left).

**Files:**
- Modify: `crates/koji-web/src/components/sparkline.rs`

**What to implement:**

1. Add a `HoverState` struct to hold hover information:
   ```rust
   #[derive(Clone, Debug)]
   struct HoverPoint {
       x_pct: f32,       // 0-100, position in the viewBox
       value: f32,       // the data value at the hovered point
       index: usize,     // index in the data array
       ts_unix_ms: i64,  // timestamp for this point (for relative time display)
   }
   ```

2. Add new props to `SparklineChart`:
   - `timestamps: Vec<i64>` — Unix ms timestamps for each data point (for relative time calculation in tooltip). If empty or len ≠ data.len, tooltip shows value only without time.
   - `unit_label: String` — e.g. "%" or "MiB" — displayed in the tooltip like "12.5%"
   - `y_refs: Vec<f32>` — Y-axis reference values to draw as dashed lines (e.g. `vec![0.0, 100.0]` for CPU, `vec![max_value]` for memory)

3. Add hover state using `RwSignal<Option<HoverPoint>>`:
   - Compute `hover_point` from cursor position on `on:mousemove`
   - The SVG viewBox is `0 0 100 {height}`. Mouse X position maps to a data index: `index = (mouse_x_pct / 100.0 * data.len() as f32).round() as usize`, clamped to `0..data.len()`
   - Set to `None` on `on:mouseleave`

4. Render inside the SVG:
   - **Y-axis reference lines**: For each value in `y_refs`, draw a horizontal dashed line at `y = height - (ref_val / safe_max * height)`, stretching from x=0 to x=100. Style: `stroke="rgba(255,255,255,0.1)" stroke-dasharray="2,2" stroke-width="0.5"`.
   - **When hovering** (`hover_point` is `Some`):
     - Vertical indicator line: `<line>` from `(x_pct, 0)` to `(x_pct, height)` with `stroke` = same color as chart, `opacity="0.4"`, `stroke-dasharray="4,2"`, `stroke-width="0.8"`
     - Highlighted dot: `<circle>` at `(x_pct, y)` with `r="2"`, `fill` = chart color, `stroke="var(--bg-secondary)"`, `stroke-width="1"`
   - **Tooltip**: A `<foreignObject>` or a positioned HTML element above the SVG showing the value and relative time. Since Leptos `view!` supports HTML alongside SVG within the same component, place a tooltip div outside the SVG but inside `.sparkline-container` using `position: absolute` with `left` and `top` derived from `hover_point`.

5. **Time axis labels**: Below the SVG, add two `<span>` elements inside `.sparkline-container`:
   - Left label: `"-{duration}"` (e.g. "-3m") if `timestamps` is non-empty, calculated from the oldest timestamp relative to `now()`; otherwise empty string
   - Right label: `"now"` if `timestamps` is non-empty
   - Style: `font-size: 0.65rem; color: var(--text-secondary); opacity: 0.6`

6. **Relative time formatting**: Create a helper function `format_relative_time(ts_unix_ms: i64) -> String`:
   - Compute diff from `js_sys::Date::now() as i64` (current browser time in ms)
   - If diff < 60_000: format as `"{seconds}s ago"`
   - If diff < 3_600_000: format as `"{minutes}m {remaining_seconds}s ago"`
   - If diff < 86_400_000: format as `"{hours}h ago"`
   - Otherwise: just the value

7. **Empty state**: When `data.is_empty()`, render the SVG with just the Y-axis reference lines (no fill path, no stroke path). Time labels should be empty.

8. **`preserveAspectRatio="none"`**: Keep current behavior so the chart stretches to fill its container.

**Important implementation notes:**
- In Leptos 0.7, `on:mousemove` event handler gets `MouseEvent`. Use `event.client_x()` and `event.client_y()` plus the SVG element's bounding rect to compute the X position within the SVG viewBox. Access the SVG element via `node_ref` or by getting `event.target()` and calling `get_bounding_client_rect()`.
- The `RwSignal<Option<HoverPoint>>` must be created inside the component function — it's local reactive state.
- The `view!` macro requires `class:name={expr}` syntax for dynamic classes and `style:name={expr}` for dynamic styles.

**Steps:**
- [ ] Run `cargo test --package koji-web` — existing tests pass
- [ ] Add `HoverState` struct and new props to `SparklineChart`
- [ ] Add hover signal, `on:mousemove`/`on:mouseleave` handlers
- [ ] Add Y-axis reference line rendering
- [ ] Add vertical indicator and dot rendering on hover
- [ ] Add tooltip rendering (value + relative time)
- [ ] Add time axis labels ("now" and "-Xm")
- [ ] Add `format_relative_time` helper
- [ ] Handle edge cases: empty data, single point, timestamps len ≠ data len
- [ ] Run `cargo test --package koji-web`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: add interactive hover, Y-axis refs, and time labels to SparklineChart"

**Acceptance criteria:**
- [ ] Hovering over sparkline shows vertical indicator line at cursor position
- [ ] Highlighted dot appears on the data line at nearest data point
- [ ] Tooltip shows exact value with unit label
- [ ] Tooltip shows relative time (e.g. "2m 15s ago") when timestamps are provided
- [ ] Y-axis reference lines render as subtle dashed lines
- [ ] Time axis labels ("now" right, "-Xm" left) appear below the sparkline
- [ ] Mouse leaving the chart clears all hover overlays
- [ ] Empty data shows Y-axis lines but no sparkline path
- [ ] All existing tests still pass

---

### Task 3: Update CSS for metric tiles layout

**Context:** The current `.card` and `.grid-stats` CSS doesn't enforce fixed height or flex alignment. Cards with long values ("8460 / 65183 MiB") are taller than cards with short values ("12.5%"), causing visual misalignment. The sparklines sit at different vertical positions. The redesign uses fixed-height cards with flex column layout so sparklines always bottom-align.

**Files:**
- Modify: `crates/koji-web/style.css`

**What to implement:**

1. Update `.grid-stats`:
   ```css
   .grid-stats {
       display: grid;
       grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
       gap: 1rem;
   }
   ```
   Change `minmax(240px, ...)` to `minmax(260px, ...)`.

2. Add `.stat-card` class for the fixed-height flex card:
   ```css
   .stat-card {
       display: flex;
       flex-direction: column;
       height: 200px;
       background-color: var(--bg-secondary);
       border: 1px solid var(--border-color);
       border-radius: var(--radius-lg);
       padding: 1.25rem;
       box-shadow: var(--shadow-card);
   }
   ```
   This replaces the generic `.card` class on stat cards (model cards still use `.card`).

3. Add `.stat-card:hover`:
   ```css
   .stat-card:hover {
       border-color: var(--border-hover);
   }
   ```

4. Update `.card-header` — no change needed, it's already fine (0.85rem uppercase).

5. Update `.card-value`:
   ```css
   .card-value {
       font-size: 1.75rem;
       font-weight: 600;
       font-family: var(--font-mono);
       color: var(--text-primary);
       line-height: 1.2;
   }
   ```
   Change from `2rem` / `700` to `1.75rem` / `600`.

6. Add `.card-secondary`:
   ```css
   .card-secondary {
       font-size: 0.75rem;
       color: var(--text-secondary);
       margin-top: 0.125rem;
       font-family: var(--font-mono);
   }
   ```

7. Update `.sparkline-container`:
   ```css
   .sparkline-container {
       position: relative;
       width: 100%;
       margin-top: auto;
       flex: 1;
       display: flex;
       flex-direction: column;
       justify-content: flex-end;
   }
   ```

8. Update `.sparkline`:
   ```css
   .sparkline {
       display: block;
       width: 100%;
       flex: 1;
       min-height: 60px;
   }
   ```
   Remove the fixed `height: 60px` and `margin-top: 0.75rem`. The sparkline now grows to fill available space via `flex: 1`.

9. Add `.sparkline-time-axis`:
   ```css
   .sparkline-time-axis {
       display: flex;
       justify-content: space-between;
       font-size: 0.65rem;
       color: var(--text-secondary);
       opacity: 0.6;
       margin-top: 0.25rem;
       padding: 0 0.25rem;
   }
   ```

10. Add `.sparkline-tooltip`:
    ```css
    .sparkline-tooltip {
        position: absolute;
        background-color: var(--bg-tertiary);
        border: 1px solid var(--border-color);
        border-radius: var(--radius-sm);
        padding: 0.35rem 0.5rem;
        font-size: 0.75rem;
        font-family: var(--font-mono);
        color: var(--text-primary);
        pointer-events: none;
        z-index: 10;
        white-space: nowrap;
        transform: translate(-50%, -100%);
        margin-top: -0.5rem;
    }
    .sparkline-tooltip-value {
        font-weight: 600;
    }
    .sparkline-tooltip-time {
        color: var(--text-secondary);
        margin-left: 0.35rem;
    }
    ```

11. Add `.card-value-empty`:
    ```css
    .card-value-empty {
       font-size: 1.75rem;
       font-weight: 600;
       font-family: var(--font-mono);
       color: var(--text-muted);
    }
    ```

12. Keep `.card--centered` and all other existing classes unchanged. The `.card` base class is still used by model cards and other sections — only stat cards get the new `.stat-card` class.

**Steps:**
- [ ] Apply all CSS changes listed above
- [ ] Verify no other pages are broken (`.card` is still the base class)
- [ ] Run `cargo build --workspace` (CSS changes don't need compilation but verify no Rust errors)
- [ ] Commit with message: "style: update CSS for fixed-height metric tiles with flex layout"

**Acceptance criteria:**
- [ ] `.stat-card` is 200px tall with flex column layout
- [ ] `.card-value` is 1.75rem / 600 weight
- [ ] `.card-secondary` exists for secondary metric info
- [ ] `.sparkline-container` uses `flex: 1` and `margin-top: auto` to push sparkline to bottom
- [ ] `.sparkline` uses `flex: 1; min-height: 60px` instead of fixed height
- [ ] `.sparkline-tooltip` styles exist for hover overlay
- [ ] `.sparkline-time-axis` styles exist for time labels
- [ ] Other pages and card styles are unchanged

---

### Task 4: Refactor dashboard card layout and add history fetch

**Context:** The dashboard renders stat cards with variable-height values and no historical data loading. This task updates the `Dashboard` component to: (1) use the new `.stat-card` layout with flex column, (2) split memory/VRAM values into primary + secondary lines, (3) load historical data on mount from the new `/koji/v1/system/metrics/history` endpoint, (4) pass timestamps and unit labels to SparklineChart, and (5) handle the empty state ("—" for values).

**Files:**
- Modify: `crates/koji-web/src/pages/dashboard.rs`

**What to implement:**

1. **Add `MetricsHistoryEntry` struct** (frontend mirror of the backend response):
   ```rust
   #[derive(Debug, Clone, Deserialize)]
   struct MetricsHistoryEntry {
       ts_unix_ms: i64,
       cpu_usage_pct: f32,
       ram_used_mib: i64,
       ram_total_mib: i64,
       gpu_utilization_pct: Option<i64>,
       vram_used_mib: Option<i64>,
       vram_total_mib: Option<i64>,
   }
   ```
   Note: Fields are `i64` to match the JSON from the backend (which uses i64 for memory values since SQLite stores them as INTEGER).

2. **Add conversion from `MetricsHistoryEntry` to `MetricSample`**:
   ```rust
   impl From<MetricsHistoryEntry> for MetricSample {
       fn from(entry: MetricsHistoryEntry) -> Self {
           MetricSample {
               ts_unix_ms: entry.ts_unix_ms,
               cpu_usage_pct: entry.cpu_usage_pct,
               ram_used_mib: entry.ram_used_mib as u64,
               ram_total_mib: entry.ram_total_mib as u64,
               gpu_utilization_pct: entry.gpu_utilization_pct.map(|v| v as u8),
               vram: entry.vram_used_mib.and_then(|used| {
                   entry.vram_total_mib.map(|total| VramInfo {
                       used_mib: used as u64,
                       total_mib: total as u64,
                   })
               }),
               models_loaded: 0, // not available in history
               models: vec![],   // not available in history
           }
       }
   }
   ```

3. **Add history fetch on mount** — in the `Effect::new` block that opens SSE, before opening the EventSource:
   ```rust
   // Fetch historical metrics before connecting to SSE
   let history_signal = history; // clone the signal for the async block
   let _ = spawn_local(async move {
       if let Ok(resp) = gloo_net::http::Request::get("/koji/v1/system/metrics/history?limit=100")
           .send()
           .await
       {
           if let Ok(entries) = resp.json::<Vec<MetricsHistoryEntry>>().await {
               let samples: Vec<MetricSample> = entries.into_iter().map(Into::into).collect();
               if !samples.is_empty() {
                   history_signal.update(|buf| {
                       *buf = samples;
                   });
               }
           }
       }
   });
   ```
   This uses `leptos::task::spawn_local` to run the async fetch. The history fetch happens before SSE connection starts, so the buffer is pre-populated with up to 100 historical samples.

4. **Update card rendering** — replace the current `<div class="card">` blocks with `<div class="stat-card">` and flex column structure:

   **CPU card example:**
   ```rust
   {
       let data = buf.iter().map(|s| s.cpu_usage_pct).collect::<Vec<f32>>();
       let timestamps = buf.iter().map(|s| s.ts_unix_ms).collect::<Vec<i64>>();
       let y_refs = vec![0.0, 100.0];
       view! {
           <div class="stat-card">
               <div class="card-header">"CPU Usage"</div>
               {match buf.last() {
                   Some(h) => view! {
                       <div class="card-value">{format!("{:.1}%", h.cpu_usage_pct)}</div>
                       <div class="card-secondary">"of 100%"</div>
                   }.into_any(),
                   None => view! {
                       <div class="card-value-empty">"—"</div>
                   }.into_any(),
               }}
               <div class="sparkline-container">
                   <SparklineChart
                       data=data
                       max_value=100.0
                       color="var(--accent-green)".to_string()
                       height=60.0
                       timestamps=timestamps
                       unit_label="%".to_string()
                       y_refs=y_refs
                   />
               </div>
           </div>
       }
   }
   ```

   **Memory card example:**
   ```rust
   {
       let data = buf.iter().map(|s| s.ram_used_mib as f32).collect::<Vec<f32>>();
       let timestamps = buf.iter().map(|s| s.ts_unix_ms).collect::<Vec<i64>>();
       let max_val = buf.last().map(|h| h.ram_total_mib as f32).unwrap_or(1.0);
       let y_refs = vec![max_val];
       view! {
           <div class="stat-card">
               <div class="card-header">"Memory"</div>
               {match buf.last() {
                   Some(h) => view! {
                       <div class="card-value">{format_number(h.ram_used_mib)}</div>
                       <div class="card-secondary">{format!("of {} MiB", format_number(h.ram_total_mib)))}</div>
                   }.into_any(),
                   None => view! {
                       <div class="card-value-empty">"—"</div>
                   }.into_any(),
               }}
               <div class="sparkline-container">
                   <SparklineChart
                       data=data
                       max_value=max_val
                       color="var(--accent-blue)".to_string()
                       height=60.0
                       timestamps=timestamps
                       unit_label="MiB".to_string()
                       y_refs=y_refs
                   />
               </div>
           </div>
       }
   }
   ```

5. **Add `format_number` helper** for large number display with commas:
   ```rust
   fn format_number(n: u64) -> String {
       let s = n.to_string();
       let mut result = String::new();
       for (i, c) in s.chars().rev().enumerate() {
           if i > 0 && i % 3 == 0 {
               result.insert(0, ',');
           }
           result.insert(0, c);
       }
       result
   }
   ```

6. **Handle the empty state**: When `buf.is_empty()` and history fetch hasn't completed yet, show all 4 stat cards with "—" values and empty sparklines (but with Y-axis refs). Don't show the "Loading..." card — instead show the grid structure with empty values. The "Loading dashboard..." fallback only shows if both history fetch AND SSE have failed.

7. **Update SSE handler** — after the history fetch call, continue opening the EventSource as before. When SSE samples arrive, append to the existing buffer (already capped at 100). When reconnecting (via `manual_refresh`), don't re-fetch history since the SSE will backfill from the new samples.

8. **GPU/VRAM card handling**: Only render GPU and VRAM cards when data is available in the **latest** sample. When available, use the same `.stat-card` flex layout. When not available, those cards are simply not rendered (no empty state card for GPU/VRAM).

9. **Update `SparklineChart` invocations** — all 4 cards now pass:
   - `timestamps`: `Vec<i64>` extracted from `buf`
   - `unit_label`: `"%"` for CPU/GPU, `"MiB"` for Memory/VRAM
   - `y_refs`: `vec![0.0, 100.0]` for percentage cards, `vec![max_value]` for memory cards

10. **The model section** (`dashboard-models`) remains unchanged — only stat cards are being redesigned.

**Steps:**
- [ ] Add `MetricsHistoryEntry` struct and `From` impl
- [ ] Add `format_number` helper function
- [ ] Add history fetch in `Effect::new` before SSE connection
- [ ] Add `leptos::task::spawn_local` import
- [ ] Add `gloo_net` import for HTTP request
- [ ] Replace `<div class="card">` with `<div class="stat-card">` for all 4 metric cards
- [ ] Split memory/VRAM values into primary + secondary lines
- [ ] Add empty state with "—" for all cards when no data
- [ ] Pass `timestamps`, `unit_label`, and `y_refs` props to SparklineChart
- [ ] Update empty/error state rendering logic
- [ ] Run `cargo test --package koji-web`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: refactor dashboard card layout with flex alignment, history fetch, and value split"

**Acceptance criteria:**
- [ ] All 4 stat cards use `.stat-card` class with fixed 200px height
- [ ] Memory/VRAM cards show primary value ("2,048") and secondary line ("of 16,384 MiB")
- [ ] CPU/GPU cards show primary value and secondary line ("of 100%")
- [ ] Empty state shows "—" for card values when no data available
- [ ] Historical data is fetched from `/koji/v1/system/metrics/history` on page load
- [ ] SSE stream appends new data to the pre-populated buffer
- [ ] GPU/VRAM cards are conditionally rendered as before
- [ ] Model cards section is unchanged
- [ ] All existing tests pass

---

### Task 5: Integration testing and visual verification

**Context:** This is the final verification task to ensure all pieces work together correctly — the backend endpoint, the frontend fetch, the CSS layout, and the sparkline interactivity.

**Files:**
- No new files. Verification only.

**What to verify:**

1. **Backend**: `cargo test --package koji-core` passes, including any existing metrics handler tests.

2. **Frontend**: `cargo test --package koji-web` passes.

3. **Full build**: `cargo build --workspace` succeeds with no errors.

4. **Clippy**: `cargo clippy --workspace -- -D warnings` passes.

5. **Manual verification checklist** (run `trunk serve` or equivalent and check):
   - Dashboard loads and fetches history from `/koji/v1/system/metrics/history`
   - If DB is empty, cards show "—" values and empty sparklines with Y-axis refs
   - If DB has data, sparklines pre-populate with up to 100 historical points
   - SSE stream continues to append new data
   - All 4 cards are fixed-height 200px, sparklines bottom-align
   - Hovering over sparkline shows vertical indicator, highlighted dot, and tooltip
   - Tooltip shows value with unit (e.g. "12.5%", "2,048 MiB") and relative time
   - Time axis labels ("now" and "-Xm") appear below sparklines
   - Y-axis reference lines show as subtle dashed lines
   - Memory/VRAM cards show split values (primary + secondary)
   - Model cards section is unaffected

**Steps:**
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all --check`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Visual review of all changes for consistency
- [ ] Commit with message: "test: verify dashboard redesign integration"

**Acceptance criteria:**
- [ ] All workspace tests pass
- [ ] Clippy passes with no warnings
- [ ] Build succeeds
- [ ] Code formatting is clean