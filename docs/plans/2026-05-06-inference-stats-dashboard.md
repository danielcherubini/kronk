# Inference Stats Dashboard Cards Plan

**Goal:** Surface llama_cpp `timings` data as 4 new stat cards on the main dashboard, updating on each non-streaming API response.

**Architecture:** The proxy intercepts `timings` from non-streaming responses, stores latest values in a `watch` channel, and emits SSE `"inference"` events. The frontend maintains a separate observation-point history buffer (not mixed into poll-driven `MetricSample`) and renders 4 sparkline cards with staleness awareness.

**Tech Stack:** Rust (tama-core proxy), Leptos WASM (tama-web frontend), SSE, tokio::watch channel

---

### Task 1: Backend — Inference stats types and ProxyState integration

**Context:**
Add the data structures and state management for tracking latest inference stats. The `LatestInferenceStats` struct holds the 4 computed metrics plus a staleness timestamp and a flag for whether spec decoding is active. A `tokio::sync::watch` channel is used (not `RwLock`) because it's single-producer (the intercept handler), multi-consumer (the metrics task), and idiomatic for "latest value" semantics.

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`
- Test: `crates/tama-core/src/proxy/types.rs` (add `#[cfg(test)]` module)

**What to implement:**

1. In `crates/tama-core/src/proxy/types.rs`, add:
   ```rust
   /// Latest inference timing stats extracted from llama_cpp response `timings` object.
   ///
   /// Stored behind a `watch` channel in `ProxyState`. Updated on each non-streaming
   /// response that includes a `timings` field. Fields are `Option<f32>` — `None` when
   /// the value cannot be computed (e.g. division by zero) or has not been observed yet.
   #[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
   pub struct LatestInferenceStats {
       /// Token generation speed (predicted_per_second from timings)
       pub tps: Option<f32>,
       /// Prompt processing speed in tokens per second (prompt_per_second from timings)
       pub prompt_tps: Option<f32>,
       /// Cache hit rate percentage (cache_n / prompt_n * 100), None if prompt_n == 0
       pub cache_hit_pct: Option<f32>,
       /// Speculative decoding acceptance rate (draft_n_accepted / draft_n * 100), None if draft_n == 0
       pub spec_accept_pct: Option<f32>,
       /// True if draft_n > 0 has ever been observed (spec decoding is active on this backend)
       pub spec_decoding_active: bool,
       /// Unix ms timestamp of the last update
       pub last_updated_ms: i64,
   }
   ```

2. In `crates/tama-core/src/proxy/types.rs`, add the `inference_stats` field to `ProxyState`:
   ```rust
   pub inference_stats: tokio::sync::watch::Sender<Option<LatestInferenceStats>>,
   ```

3. In `crates/tama-core/src/proxy/state.rs`, in `ProxyState::new`:
   - Initialize: `inference_stats: tokio::sync::watch::channel(None).0`
   - Add to the struct construction alongside other fields

4. In `crates/tama-core/src/proxy/types.rs`, in `ProxyState::shutdown`:
   - Clear: `let _ = self.inference_stats.send(None);`

5. Add tests in `types.rs`:
   - `test_latest_inference_stats_default` — verify Default produces all `None` / `false` / `0`
   - `test_latest_inference_stats_clone_copy` — verify Copy and Clone work
   - `test_inference_stats_watch_round_trip` — create `watch::channel(None)`, send `Some(LatestInferenceStats { ... })`, subscribe, verify received value matches. Tests the core state mechanism end-to-end.

**Steps:**
- [ ] Write test `test_latest_inference_stats_default` in `crates/tama-core/src/proxy/types.rs`
- [ ] Implement `LatestInferenceStats` struct with derives
- [ ] Run `cargo test --package tama-core test_latest_inference_stats`
  - Did all tests pass? If not, fix and re-run.
- [ ] Add `inference_stats` field to `ProxyState` struct in `types.rs`
- [ ] Initialize the watch channel in `ProxyState::new` in `state.rs`
- [ ] Clear inference stats in `ProxyState::shutdown` in `types.rs`
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix warnings.
- [ ] Run `cargo fmt --package tama-core`
- [ ] Commit with message: "feat: add LatestInferenceStats type and ProxyState integration"

**Acceptance criteria:**
- [ ] `LatestInferenceStats` struct exists with all 6 fields, derives `Debug, Clone, Copy, Default, Serialize, Deserialize`
- [ ] `ProxyState` has `inference_stats: watch::Sender<Option<LatestInferenceStats>>` field
- [ ] Channel initialized to `None` in `ProxyState::new`
- [ ] Channel cleared to `None` in `ProxyState::shutdown`
- [ ] `cargo test --package tama-core` passes
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 2: Backend — Extract timings from forwarded responses

**Context:**
The proxy's `forward_request` already parses non-streaming JSON responses to rewrite the `model` field. Extend this code path to also extract the `timings` object, compute derived fields (cache hit %, spec accept %), and send via the watch channel. If `timings` is absent or malformed, preserve last known values (do not update). Division by zero produces `None` for that field.

**Files:**
- Modify: `crates/tama-core/src/proxy/forward.rs`
- Test: `crates/tama-core/src/proxy/forward.rs` (add tests to existing `#[cfg(test)]` module)

**What to implement:**

1. Add a helper function (can be in `forward.rs` or a new module):
   ```rust
   /// Extract inference stats from a llama_cpp `timings` object in a JSON response.
   ///
   /// Returns `None` if the response has no `timings` field or it cannot be parsed.
   /// Returns `Some(LatestInferenceStats)` with computed fields otherwise.
   /// Division by zero (prompt_n == 0, draft_n == 0) produces `None` for that field.
   fn extract_inference_stats(json: &serde_json::Value) -> Option<LatestInferenceStats> {
       let timings = json.get("timings")?;
       // Extract fields with defaults of 0 for missing numeric fields
       let predicted_per_second = timings.get("predicted_per_second")?.as_f64()?;
       let prompt_per_second = timings.get("prompt_per_second")?.as_f64()?;
       let cache_n = timings.get("cache_n").and_then(|v| v.as_u64()).unwrap_or(0);
       let prompt_n = timings.get("prompt_n").and_then(|v| v.as_u64()).unwrap_or(0);
       let draft_n = timings.get("draft_n").and_then(|v| v.as_u64()).unwrap_or(0);
       let draft_n_accepted = timings.get("draft_n_accepted").and_then(|v| v.as_u64()).unwrap_or(0);

       let now_ms = std::time::SystemTime::now()
           .duration_since(std::time::UNIX_EPOCH)
           .unwrap_or_default()
           .as_millis() as i64;

       Some(LatestInferenceStats {
           tps: Some(predicted_per_second as f32),
           prompt_tps: Some(prompt_per_second as f32),
           cache_hit_pct: if prompt_n > 0 {
               Some((cache_n as f32 / prompt_n as f32 * 100.0).clamp(0.0, 100.0))
           } else {
               None
           },
           spec_accept_pct: if draft_n > 0 {
               Some((draft_n_accepted as f32 / draft_n as f32 * 100.0).clamp(0.0, 100.0))
           } else {
               None
           },
           spec_decoding_active: draft_n > 0,
           last_updated_ms: now_ms,
       })
   }
   ```

2. In `forward_request`, modify the existing non-streaming JSON block to extract inference stats from the already-parsed value (avoid double parsing). Replace the current non-streaming block:
   ```rust
   // Non-streaming response - parse, rewrite, and re-serialize
   let body_bytes = response.bytes().await.unwrap_or_default();
   // Only attempt JSON rewrite if content is valid JSON
   let new_body = if let Ok(parsed) = serde_json::from_slice::<JsonValue>(&body_bytes) {
       // Extract inference stats from timings (before rewrite — timings unaffected by model name change)
       if let Some(stats) = extract_inference_stats(&parsed, &state.inference_stats) {
           state.inference_stats.send_replace(Some(stats));
       }
       let rewritten = rewrite_json_model_name(parsed, model_name);
       serde_json::to_vec(&rewritten).unwrap_or(body_bytes.to_vec())
   } else {
       // Not JSON, pass through unchanged
       body_bytes.to_vec()
   };
   ```
   - Extract from the already-parsed `JsonValue` — no second parse needed
   - Use `send_replace()` (always stores the value, even if no receivers are subscribed)
   - Do NOT clear existing stats if `timings` is absent (only update when present)

   Update `extract_inference_stats` signature to accept the watch sender so it can read the previous `spec_decoding_active` flag:
   ```rust
   fn extract_inference_stats(
       json: &serde_json::Value,
       inference_stats: &tokio::sync::watch::Sender<Option<LatestInferenceStats>>,
   ) -> Option<LatestInferenceStats> {
       let timings = json.get("timings")?;
       // ...
       let prev_active = inference_stats.borrow().and_then(|s| Some(s.spec_decoding_active)).unwrap_or(false);
       // ...
       Some(LatestInferenceStats {
           // ...
           spec_decoding_active: draft_n > 0 || prev_active, // sticky: once true, stays true
           // ...
       })
   }
   ```

3. Add tests:
   - `test_extract_inference_stats_full_timings` — all fields present, computes correctly
   - `test_extract_inference_stats_missing_timings` — no `timings` key → `None`
   - `test_extract_inference_stats_zero_prompt_n` — `prompt_n: 0` → `cache_hit_pct: None`
   - `test_extract_inference_stats_zero_draft_n` — `draft_n: 0` → `spec_accept_pct: None`, `spec_decoding_active: false`
   - `test_extract_inference_stats_partial_timings` — some fields missing → uses defaults

**Steps:**
- [ ] Write failing test `test_extract_inference_stats_full_timings` in `forward.rs`
- [ ] Implement `extract_inference_stats` helper function
- [ ] Run `cargo test --package tama-core test_extract_inference_stats`
  - Did all tests pass? If not, fix and re-run.
- [ ] Add the inference stats extraction call in `forward_request` (non-streaming branch)
- [ ] Write remaining tests (missing timings, zero prompt_n, zero draft_n, partial)
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo fmt --package tama-core`
- [ ] Commit with message: "feat: extract inference timings from forwarded responses"

**Acceptance criteria:**
- [ ] `extract_inference_stats` function exists and handles all edge cases
- [ ] Non-streaming responses with `timings` update `inference_stats` watch channel
- [ ] Missing/malformed `timings` does NOT update (preserves last known)
- [ ] Division by zero produces `None` for the affected field
- [ ] `spec_decoding_active` is `true` only when `draft_n > 0`
- [ ] All tests pass, clippy clean

---

### Task 3: Backend — SSE inference event emission

**Context:**
The metrics stream handler (`handle_system_metrics_stream`) currently only emits `"sample"` events from the broadcast channel. Add a second mechanism: subscribe to the `inference_stats` watch channel and emit `"inference"` SSE events whenever the value changes. This keeps inference stats separate from poll-driven system metrics.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/system.rs`

**What to implement:**

1. In `handle_system_metrics_stream`, after creating the broadcast channel subscriber, also create a watch channel subscriber:
   ```rust
   let mut inference_rx = state.inference_stats.subscribe();
   ```

2. Modify the async stream to handle both event types. The current `async_stream::stream!` loop only handles the broadcast channel. Restructure to use `tokio::select!` with a guard flag for the watch channel:
   ```rust
   let stream = async_stream::stream! {
       let mut inference_active = true;
       loop {
           tokio::select! {
               // System metrics samples (broadcast channel)
               result = rx.recv() => {
                   match result {
                       Ok(sample) => {
                           match serde_json::to_string(&sample) {
                               Ok(data) => yield Ok(Event::default().event("sample").data(data)),
                               Err(e) => tracing::warn!("failed to serialize MetricSample: {}", e),
                           }
                       }
                       Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                           let data = format!("{{\"missed\":{}}}", n);
                           yield Ok(Event::default().event("lagged").data(data));
                       }
                       Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                   }
               }
               // Inference stats updates (watch channel)
               // `if inference_active` guard: once the watch channel closes, stop polling it
               // but keep the broadcast channel alive for system metrics
               result = inference_rx.changed(), if inference_active => {
                   match result {
                       Ok(()) => {
                           // changed() resolved — read the latest value
                           let value = inference_rx.borrow_and_update().clone();
                           match value {
                               Some(stats) => {
                                   match serde_json::to_string(&stats) {
                                       Ok(data) => yield Ok(Event::default().event("inference").data(data)),
                                       Err(e) => tracing::warn!("failed to serialize LatestInferenceStats: {}", e),
                                   }
                               }
                               None => {
                                   // Stats cleared (e.g. shutdown) — emit empty
                                   yield Ok(Event::default().event("inference").data("null"));
                               }
                           }
                       }
                       Err(_) => {
                           // Watch channel closed — stop emitting inference events
                           // System metrics continue via the broadcast channel
                           inference_active = false;
                       }
                   }
               }
           }
       }
   };
   ```

   **Key API notes:**
   - `watch::Receiver` uses `changed()` (returns `Result<(), RecvError>`), NOT `recv()`
   - After `changed()` resolves, use `borrow_and_update()` to read the value (avoids race between `changed()` resolving and value being read)
   - The `if inference_active` guard on the select branch prevents polling a closed watch channel while keeping the broadcast channel alive

**Steps:**
- [ ] Modify `handle_system_metrics_stream` to subscribe to `inference_stats` watch channel
- [ ] Implement `tokio::select!` loop handling both broadcast and watch channels
- [ ] Emit `"inference"` SSE events with JSON-serialized `LatestInferenceStats`
- [ ] Handle watch channel close gracefully (stop inference events, keep system metrics)
- [ ] Write test `test_latest_inference_stats_serialization` — verify JSON shape matches frontend expectations (all fields present, correct types)
- [ ] Run `cargo test --package tama-core test_latest_inference_stats`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo fmt --package tama-core`
- [ ] Commit with message: "feat: emit inference stats as SSE events on metrics stream"

**Acceptance criteria:**
- [ ] SSE stream at `/tama/v1/system/metrics/stream` emits `"inference"` events
- [ ] Inference events contain JSON-serialized `LatestInferenceStats`
- [ ] System metrics `"sample"` events still work (no regression)
- [ ] Watch channel close doesn't kill the entire SSE stream (system metrics continue)
- [ ] `LatestInferenceStats` serializes to valid JSON with all 6 fields
- [ ] `cargo build` and `cargo clippy` pass

---

### Task 4: Frontend — Inference stats types and SSE listener

**Context:**
Add the frontend data structures and SSE listener for inference stats. The frontend maintains a separate history buffer (not mixed into `MetricSample`) so sparklines plot actual observation points, not repeated poll-driven values. Staleness is detected by comparing `last_updated_ms` against current time.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/metrics.rs`
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs`

**What to implement:**

1. In `crates/tama-web/src/pages/dashboard/metrics.rs`, add:
   ```rust
   #[derive(Debug, Clone, Default, Serialize, Deserialize)]
   #[serde(default)]
   pub struct InferenceStats {
       #[serde(default)]
       pub tps: Option<f32>,
       #[serde(default)]
       pub prompt_tps: Option<f32>,
       #[serde(default)]
       pub cache_hit_pct: Option<f32>,
       #[serde(default)]
       pub spec_accept_pct: Option<f32>,
       #[serde(default)]
       pub spec_decoding_active: bool,
       #[serde(default)]
       pub last_updated_ms: i64,
   }
   ```
   - `#[serde(default)]` on struct and fields for forward compatibility (matches existing `MetricSample` pattern)

2. In the `Dashboard` component (`mod.rs`), add:
   ```rust
   let inference_history = RwSignal::new(Vec::<InferenceStats>::new());
   let inference_max_len = 450;
   ```

3. In the SSE `Effect::new` block (where `sample` and `lagged` events are handled), add a handler for `"inference"` events:
   ```rust
   let on_inference =
       Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
           if let Some(data_str) = evt.data().as_string() {
               if data_str == "null" {
                   inference_history.set(Vec::new());
                   return;
               }
               if let Ok(stats) = serde_json::from_str::<InferenceStats>(&data_str) {
                   inference_history.update(|buf| {
                       buf.push(stats);
                       if buf.len() > inference_max_len {
                           buf.drain(..buf.len() - inference_max_len);
                       }
                   });
               }
           }
       });
   let _ = es.add_event_listener_with_callback("inference", on_inference.as_ref().unchecked_ref());
   on_inference.forget();
   ```

4. Add a helper function for staleness check:
   ```rust
   /// Returns true if the stats are considered stale (>30s old)
   fn is_stale(last_updated_ms: i64) -> bool {
       let now = js_sys::Date::now() as i64;
       (now - last_updated_ms) > 30_000
   }
   ```

5. Add a helper for formatting relative time (reuse from sparkline.rs or create new):
   ```rust
   /// Format staleness as "Xs ago" / "Xm ago"
   fn format_stale_time(last_updated_ms: i64) -> String {
       let now = js_sys::Date::now() as i64;
       let diff_secs = (now - last_updated_ms) / 1_000;
       if diff_secs < 60 {
           format!("{}s ago", diff_secs)
       } else {
           format!("{}m ago", diff_secs / 60)
       }
   }
   ```

**Steps:**
- [ ] Add `InferenceStats` struct to `metrics.rs`
- [ ] Add `inference_history` signal to Dashboard component
- [ ] Add SSE `"inference"` event handler in the Effect block
- [ ] Add `is_stale` and `format_stale_time` helper functions
- [ ] Run `cargo build --package tama-web --target wasm32-unknown-unknown`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo fmt --package tama-web`
- [ ] Commit with message: "feat: add frontend inference stats types and SSE listener"

**Acceptance criteria:**
- [ ] `InferenceStats` struct exists with all 6 fields
- [ ] `inference_history` signal stores up to 450 entries
- [ ] SSE `"inference"` events are parsed and pushed to history
- [ ] `"null"` events clear the history
- [ ] `is_stale` returns true when `last_updated_ms` > 30s old
- [ ] `cargo build` for wasm32 target passes

---

### Task 5: Frontend — Dashboard stat cards with sparklines

**Context:**
Add 4 new stat cards to the dashboard's `.grid-stats` grid, after the existing VRAM card. Each card shows the latest value, a sparkline chart from the observation-point history, and a staleness indicator. The Spec Accept card only renders when `spec_decoding_active` is true.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs`
- Modify: `crates/tama-web/css/15-dashboard.css` (optional, for card-specific styling)

**What to implement:**

1. In the `Dashboard` component's view, after the existing VRAM card block, add a conditional block for inference cards:
   ```rust
   // Inference stats cards — only render when we have data
   {move || {
       let buf = inference_history.get();
       if buf.is_empty() {
           return view! { <div></div> }.into_any();
       }

       let latest = buf.last().cloned().unwrap();
       let stale = is_stale(latest.last_updated_ms);
       let timestamps: Vec<i64> = buf.iter().map(|s| s.last_updated_ms).collect();

       // Extract sparkline data (use 0 for None values in the chart, but show "—" in the label)
       let tps_data: Vec<f32> = buf.iter().map(|s| s.tps.unwrap_or(0.0)).collect();
       let prompt_tps_data: Vec<f32> = buf.iter().map(|s| s.prompt_tps.unwrap_or(0.0)).collect();
       let cache_data: Vec<f32> = buf.iter().map(|s| s.cache_hit_pct.unwrap_or(0.0)).collect();
       let spec_data: Vec<f32> = buf.iter().map(|s| s.spec_accept_pct.unwrap_or(0.0)).collect();

       // Determine max values for sparkline scaling
       let tps_max = tps_data.iter().cloned().fold(1.0f32, f32::max);
       let prompt_tps_max = prompt_tps_data.iter().cloned().fold(1.0f32, f32::max);

       view! {
           <div class="grid-stats">
               // Processing Speed card
               <div class="stat-card" class:stat-card--stale=stale>
                   <div class="card-header">"Processing Speed"</div>
                   {match latest.prompt_tps {
                       Some(v) => view! {
                           <div class="card-value">{format!("{:.1} tok/s", v)}</div>
                           <div class="card-secondary">{format_stale_time(latest.last_updated_ms)}</div>
                       }.into_any(),
                       None => view! {
                           <div class="card-value-empty">"—"</div>
                       }.into_any(),
                   }}
                   <div class="sparkline-container">
                       <SparklineChart
                           data=prompt_tps_data
                           max_value=prompt_tps_max
                           color="var(--accent-orange)".to_string()
                           height=60.0
                           timestamps=timestamps.clone()
                           unit_label="tok/s".to_string()
                           y_refs=vec![]
                       />
                   </div>
               </div>

               // Gen Speed card
               <div class="stat-card" class:stat-card--stale=stale>
                   <div class="card-header">"Gen Speed"</div>
                   {match latest.tps {
                       Some(v) => view! {
                           <div class="card-value">{format!("{:.1} tok/s", v)}</div>
                           <div class="card-secondary">{format_stale_time(latest.last_updated_ms)}</div>
                       }.into_any(),
                       None => view! {
                           <div class="card-value-empty">"—"</div>
                       }.into_any(),
                   }}
                   <div class="sparkline-container">
                       <SparklineChart
                           data=tps_data
                           max_value=tps_max
                           color="var(--accent-cyan)".to_string()
                           height=60.0
                           timestamps=timestamps.clone()
                           unit_label="tok/s".to_string()
                           y_refs=vec![]
                       />
                   </div>
               </div>

               // Cache Hits card
               <div class="stat-card" class:stat-card--stale=stale>
                   <div class="card-header">"Cache Hits"</div>
                   {match latest.cache_hit_pct {
                       Some(v) => view! {
                           <div class="card-value">{format!("{:.1}%", v)}</div>
                           <div class="card-secondary">{format_stale_time(latest.last_updated_ms)}</div>
                       }.into_any(),
                       None => view! {
                           <div class="card-value-empty">"—"</div>
                       }.into_any(),
                   }}
                   <div class="sparkline-container">
                       <SparklineChart
                           data=cache_data
                           max_value=100.0
                           color="var(--accent-green)".to_string()
                           height=60.0
                           timestamps=timestamps.clone()
                           unit_label="%".to_string()
                           y_refs=vec![0.0, 100.0]
                       />
                   </div>
               </div>

               // Spec Accept card — only render when spec decoding is active
               {if latest.spec_decoding_active {
                   view! {
                       <div class="stat-card" class:stat-card--stale=stale>
                           <div class="card-header">"Spec Accept"</div>
                           {match latest.spec_accept_pct {
                               Some(v) => view! {
                                   <div class="card-value">{format!("{:.1}%", v)}</div>
                                   <div class="card-secondary">{format_stale_time(latest.last_updated_ms)}</div>
                               }.into_any(),
                               None => view! {
                                   <div class="card-value-empty">"—"</div>
                               }.into_any(),
                           }}
                           <div class="sparkline-container">
                               <SparklineChart
                                   data=spec_data
                                   max_value=100.0
                                   color="var(--accent-pink)".to_string()
                                   height=60.0
                                   timestamps=timestamps
                                   unit_label="%".to_string()
                                   y_refs=vec![0.0, 100.0]
                               />
                           </div>
                       </div>
                   }.into_any()
               } else {
                   view! { <div></div> }.into_any()
               }}
           </div>
       }.into_any()
   }}
   ```

2. In `crates/tama-web/css/15-dashboard.css`, add:
   ```css
   /* Stale inference stat cards — dimmed appearance */
   .stat-card--stale {
       opacity: 0.5;
   }
   ```

3. In `css/01-custom-properties.css`, add `--accent-pink` if missing (required — Spec Accept card uses it):
   ```css
   --accent-pink: #f472b6;
   ```
   - `--accent-cyan` and `--accent-orange` already exist (do NOT overwrite existing values)

4. Note on grid layout: The inference cards render in a **separate** `.grid-stats` div below the system metrics grid. This is intentional — system metrics always render (4 cards), inference cards are conditional (appear after first non-streaming request). Two grids is cleaner than restructuring the existing closure to merge them.

**Steps:**
- [ ] Add inference stat cards to the Dashboard view (after VRAM card)
- [ ] Add `stat-card--stale` CSS class
- [ ] Verify accent colors exist in custom properties CSS (add if missing)
- [ ] Run `cargo build --package tama-web --target wasm32-unknown-unknown`
  - Did it succeed? If not, fix compilation errors.
- [ ] Run `cargo fmt --package tama-web`
- [ ] Commit with message: "feat: add inference stats dashboard cards with sparklines"

**Acceptance criteria:**
- [ ] 4 stat cards render when inference history is non-empty
- [ ] Cards are hidden when history is empty (no inference requests yet)
- [ ] Processing Speed shows `X.X tok/s`, Gen Speed shows `X.X tok/s`, Cache Hits shows `X.X%`, Spec Accept shows `X.X%`
- [ ] Spec Accept card only renders when `spec_decoding_active` is true
- [ ] Cards dim (opacity 0.5) when `last_updated_ms` > 30s old
- [ ] Secondary text shows relative time ("Xs ago" / "Xm ago")
- [ ] Sparklines use correct colors and data
- [ ] Cards wrap to a second row in the grid (4 columns)
- [ ] `cargo build` passes

---

## Notes

- **No DB migration needed** — inference stats are ephemeral, not persisted
- **No `MetricsHistoryEntry` changes** — history endpoint returns system metrics only
- **Frontend backward compatible** — old backends don't emit `"inference"` events; frontend simply shows no cards
- **Backend backward compatible** — old frontends ignore unknown SSE event types
- **Display formatting**: `tok/s` → 1 decimal, `%` → 1 decimal
- **Sparkline colors**: orange (Processing Speed), cyan (Gen Speed), green (Cache Hits), pink (Spec Accept)
- **Multi-model scenario**: If multiple models serve requests concurrently, the watch channel shows the last response's stats (last-write-wins). No per-model breakdown — a future iteration could add a `model_name` field
- **Two grid design**: System metrics and inference stats render in separate `.grid-stats` divs. System metrics always present; inference cards appear conditionally after first non-streaming request
