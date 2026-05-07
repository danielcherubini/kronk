# Metrics Snapshot Stream

**Goal:** Replace delta-based SSE metrics streaming with full snapshot delivery every 2s, unify inference stats into the same pipeline, and eliminate frontend desync issues.

**Architecture:** The metrics task maintains an in-memory `VecDeque<MetricSample>` (450 entries, seeded from SQLite on startup). Each 2s tick it appends a new unified sample (system + inference + models) and broadcasts the full buffer. The SSE handler emits a single `event: snapshot` with a JSON array. The frontend replaces its buffer on each snapshot — always 100% in sync, no backfill, no merge logic.

**Tech Stack:** Rust (tama-core, tama-web), SQLite, Leptos (WASM), SSE

---

## Pre-work: Read these files before any task

- `crates/tama-core/src/gpu/system.rs` — `MetricSample`, `ModelStatus`, `SystemMetrics`
- `crates/tama-core/src/db/queries/metrics_queries.rs` — `SystemMetricsRow`, insert/query functions
- `crates/tama-core/src/db/migrations.rs` — migration system, `LATEST_VERSION = 21`
- `crates/tama-core/src/proxy/server/mod.rs` — metrics task in `ProxyServer::new`
- `crates/tama-core/src/proxy/tama_handlers/system.rs` — SSE handler, history endpoint
- `crates/tama-core/src/proxy/types.rs` — `ProxyState` with `metrics_tx`, `inference_stats`
- `crates/tama-web/src/pages/dashboard/mod.rs` — Dashboard component
- `crates/tama-web/src/pages/dashboard/metrics.rs` — Frontend types, backfill, merge

---

### Task 1: Database — add inference columns + migration v22

**Context:**
The `system_metrics_history` table currently stores only system metrics (CPU, RAM, GPU, VRAM). We need to add inference stat columns so that a single sample contains all dashboard data. This is a pure schema change — existing rows get `NULL` for the new columns, which is the correct semantics (no inference data available).

**Files:**
- Modify: `crates/tama-core/src/db/migrations.rs`
- Modify: `crates/tama-core/src/db/queries/metrics_queries.rs`

**What to implement:**

1. In `migrations.rs`:
   - Increment `LATEST_VERSION` from 21 to 22
   - Add migration entry for v22 with four ALTER TABLE statements:
     ```sql
     ALTER TABLE system_metrics_history ADD COLUMN tps REAL;
     ALTER TABLE system_metrics_history ADD COLUMN prompt_tps REAL;
     ALTER TABLE system_metrics_history ADD COLUMN cache_hit_pct REAL;
     ALTER TABLE system_metrics_history ADD COLUMN spec_accept_pct REAL;
     ```

2. In `metrics_queries.rs`:
   - Add fields to `SystemMetricsRow`:
     ```rust
     pub tps: Option<f64>,
     pub prompt_tps: Option<f64>,
     pub cache_hit_pct: Option<f64>,
     pub spec_accept_pct: Option<f64>,
     ```
   - Update `insert_system_metric`: add 4 new columns to INSERT statement, pass 4 new params (rows 9-12)
   - Update `get_system_metrics_since`: add 4 new columns to SELECT, add 4 new `row.get()` calls (indices 8-11)
   - Update `get_recent_system_metrics`: same SELECT + row.get changes
   - Update all `#[cfg(test)]` tests:
     - `test_conn()`: add 4 new columns to CREATE TABLE
     - `make_row()`: add 4 new fields (all `None` by default)
     - Any test that constructs `SystemMetricsRow` directly: add the 4 fields

**Steps:**
- [ ] Write a test in `metrics_queries.rs` `#[cfg(test)]` that verifies the 4 new columns exist in the test schema and are queryable
- [ ] Run `cargo test --package tama-core db::queries::metrics_queries -- --nocapture`
  - Did it fail? If the test creates a schema without the new columns, it should fail. If not, adjust.
- [ ] Add migration v22 in `migrations.rs`
- [ ] Update `SystemMetricsRow` struct with 4 new fields
- [ ] Update `insert_system_metric` with new columns
- [ ] Update `get_system_metrics_since` with new columns
- [ ] Update `get_recent_system_metrics` with new columns
- [ ] Update all test helpers and test cases
- [ ] Run `cargo test --package tama-core db::queries::metrics_queries`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat(db): add inference columns to system_metrics_history (migration v22)"

**Acceptance criteria:**
- [ ] `LATEST_VERSION` is 22
- [ ] `SystemMetricsRow` has `tps`, `prompt_tps`, `cache_hit_pct`, `spec_accept_pct` fields (all `Option<f64>`)
- [ ] All three query functions (insert, since, recent) include the 4 new columns
- [ ] All existing tests pass, plus new test for columns
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 2: Backend — MetricSample fields + broadcast type change

**Context:**
Add inference fields to `MetricSample` and change the broadcast channel type from `Sender<MetricSample>` to `Sender<Arc<[MetricSample]>>`. Using `Arc` avoids deep-cloning ~450KB of data every 2s per subscriber — each subscriber only clones an 8-byte Arc reference. The SSE handler will be rewritten in Task 3 to match this new type, so this task and Task 3 share a single commit.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs`
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`

**What to implement:**

1. In `gpu/system.rs` — `MetricSample` struct:
   - Add fields with per-field `#[serde(default)]` (NOT struct-level, to avoid silently accepting malformed payloads for existing fields):
     ```rust
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
     pub inference_last_updated_ms: Option<i64>,
     ```
   - Note: `spec_decoding_active` and `inference_last_updated_ms` are transient — not persisted to SQLite. `spec_decoding_active` is a momentary state flag; `inference_last_updated_ms` tracks staleness relative to wall-clock. Neither is meaningful for historical data loaded on restart.

2. In `proxy/types.rs` — `ProxyState`:
   - Change `metrics_tx` type from `broadcast::Sender<crate::gpu::MetricSample>` to `broadcast::Sender<std::sync::Arc<[crate::gpu::MetricSample]>>`
   - Change the `shutdown()` sentinel send to send an empty Arc: `Arc::<[crate::gpu::MetricSample]>::new([])`

3. In `proxy/state.rs` — `ProxyState::new`:
   - Change `broadcast::channel(64)` to `broadcast::channel(3)` (capacity 3 gives a small safety margin for ~450KB payloads)

**Steps:**
- [ ] Add 6 inference fields to `MetricSample` in `gpu/system.rs` with per-field `#[serde(default)]`
- [ ] Change `metrics_tx` type in `proxy/types.rs` to `Sender<Arc<[MetricSample]>>`
- [ ] Update `shutdown()` sentinel in `proxy/types.rs` to send empty Arc
- [ ] Change broadcast channel capacity to 3 in `proxy/state.rs`
- [ ] Fix all compilation errors across the crate (this will break the SSE handler and metrics task — that's expected, Task 3 fixes them)
- [ ] Run `cargo check --package tama-core` — expect errors in SSE handler and metrics task (unfixed by design)
- [ ] **Do NOT commit yet** — this task is atomic with Task 3. Continue to Task 3, then commit both together.

**Acceptance criteria:**
- [ ] `MetricSample` has 6 new inference fields with per-field `#[serde(default)]`
- [ ] `metrics_tx` is `broadcast::Sender<Arc<[MetricSample]>>` with capacity 3
- [ ] `shutdown()` sends empty Arc slice
- [ ] Code does not compile (expected — Task 3 completes the change)

---

### Task 3: Backend — metrics task + SSE handler rewrite (atomic with Task 2)

**Context:**
Rewrite the metrics task to maintain a `VecDeque<MetricSample>` buffer (450 entries, seeded from SQLite) and broadcast `Arc<[MetricSample]>` each tick. Rewrite the SSE handler to emit `event: snapshot` with a JSON array. This task MUST be committed together with Task 2 — the broadcast type change breaks the SSE handler until this task fixes it.

**Files:**
- Modify: `crates/tama-core/src/proxy/server/mod.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/system.rs`
- Modify: `crates/tama-core/src/proxy/server/router.rs`

**What to implement:**

1. In `proxy/server/mod.rs` — metrics task in `ProxyServer::new`:
   - Add `use std::collections::VecDeque;` and `use std::sync::Arc;`
   - Before the main loop, seed the buffer from SQLite:
     ```rust
     let mut history_buf: VecDeque<crate::gpu::MetricSample> = VecDeque::with_capacity(450);
     if let Some(seed_conn) = state.open_db() {
         if let Ok(rows) = crate::db::queries::get_recent_system_metrics(&seed_conn, 450) {
             for row in rows {
                 history_buf.push_back(row_into_sample(&row));
             }
         }
     }
     ```
   - Define `row_into_sample` — converts `SystemMetricsRow` to `MetricSample`:
     ```rust
     fn row_into_sample(row: &crate::db::queries::SystemMetricsRow) -> crate::gpu::MetricSample {
         crate::gpu::MetricSample {
             ts_unix_ms: row.ts_unix_ms,
             cpu_usage_pct: row.cpu_usage_pct,
             ram_used_mib: row.ram_used_mib.max(0) as u64,
             ram_total_mib: row.ram_total_mib.max(0) as u64,
             gpu_utilization_pct: row.gpu_utilization_pct.and_then(|v| if v >= 0 && v <= 100 { Some(v as u8) } else { None }),
             vram: row.vram_used_mib.and_then(|used| {
                 row.vram_total_mib.map(|total| crate::gpu::VramInfo {
                     used_mib: used.max(0) as u64,
                     total_mib: total.max(0) as u64,
                 })
             }),
             models_loaded: row.models_loaded.max(0) as u64,
             models: vec![], // Not stored in DB — seeded samples have no model status
             tps: row.tps.map(|v| v as f32),
             prompt_tps: row.prompt_tps.map(|v| v as f32),
             cache_hit_pct: row.cache_hit_pct.map(|v| v as f32),
             spec_accept_pct: row.spec_accept_pct.map(|v| v as f32),
             spec_decoding_active: false, // Transient — not in DB
             inference_last_updated_ms: None, // Transient — not in DB
         }
     }
     ```
   - In the main loop, **correct order** (inference read BEFORE sample construction):
     ```rust
     // 1. Collect system metrics (spawn_blocking, unchanged)
     let (snapshot, returned_sys) = tokio::task::spawn_blocking(move || {
         let snapshot = crate::gpu::collect_system_metrics_with(&mut sys);
         (snapshot, sys)
     }).await.unwrap_or_else(|e| {
         tracing::warn!("system metrics collection panicked: {}", e);
         (crate::gpu::SystemMetrics::default(), sysinfo::System::new())
     });
     sys = returned_sys;

     // 2. Read latest inference stats from watch channel
     let inference = *metrics_state.inference_stats.borrow();

     // 3. Collect model statuses
     let model_statuses = metrics_state.collect_model_statuses().await;
     let models_loaded = model_statuses.iter().filter(|m| m.state == "ready").count() as u64;

     // 4. Build unified MetricSample WITH inference fields
     let sample = crate::gpu::MetricSample {
         ts_unix_ms,
         cpu_usage_pct: snapshot.cpu_usage_pct,
         ram_used_mib: snapshot.ram_used_mib,
         ram_total_mib: snapshot.ram_total_mib,
         gpu_utilization_pct: snapshot.gpu_utilization_pct,
         vram: snapshot.vram.clone(),
         models_loaded,
         models: model_statuses,
         tps: inference.as_ref().and_then(|i| i.tps),
         prompt_tps: inference.as_ref().and_then(|i| i.prompt_tps),
         cache_hit_pct: inference.as_ref().and_then(|i| i.cache_hit_pct),
         spec_accept_pct: inference.as_ref().and_then(|i| i.spec_accept_pct),
         spec_decoding_active: inference.map(|i| i.spec_decoding_active).unwrap_or(false),
         inference_last_updated_ms: inference.as_ref().and_then(|i| i.last_updated_ms),
     };

     // 5. Persist to SQLite (include inference fields in SystemMetricsRow)
     let row = crate::db::queries::SystemMetricsRow {
         // ... existing fields ...
         tps: sample.tps.map(|v| v as f64),
         prompt_tps: sample.prompt_tps.map(|v| v as f64),
         cache_hit_pct: sample.cache_hit_pct.map(|v| v as f64),
         spec_accept_pct: sample.spec_accept_pct.map(|v| v as f64),
     };
     // ... persist (spawn_blocking, unchanged) ...

     // 6. Update in-memory buffer
     history_buf.push_back(sample);
     while history_buf.len() > 450 {
         history_buf.pop_front(); // O(1)
     }

     // 7. Broadcast as Arc slice (no deep clone)
     let _ = metrics_state.metrics_tx.send(history_buf.make_contiguous().as_slice().into());
     ```
   - Update all tests in the file that interact with `metrics_tx`:
     - `test_metrics_task_broadcasts_samples`: expect `Arc<[MetricSample]>`, check `rx.recv()` returns `Ok(arc)`, assert `!arc.is_empty()`
     - `test_metric_sample_broadcast_populates_models_field`: same

2. In `proxy/tama_handlers/system.rs` — SSE handler:
   - Rewrite `handle_system_metrics_stream`:
     ```rust
     pub async fn handle_system_metrics_stream(
         State(state): State<Arc<ProxyState>>,
     ) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
         let mut rx = state.metrics_tx.subscribe();
         let stream = async_stream::stream! {
             loop {
                 match rx.recv().await {
                     Ok(samples) => {
                         if samples.is_empty() { break; } // Shutdown sentinel
                         match serde_json::to_string(&samples) {
                             Ok(data) => yield Ok(Event::default().event("snapshot").data(data)),
                             Err(e) => tracing::warn!("failed to serialize MetricSample slice: {}", e),
                         }
                     }
                     Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                         // Subscriber lagged — next snapshot will have full history, no action needed
                         continue;
                     }
                     Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                 }
             }
         };
         Sse::new(stream).keep_alive(KeepAlive::default())
     }
     ```
   - Remove: `inference_rx` watch channel subscription, `inference_active` guard, `tokio::select!`, `event: sample`, `event: inference`, `event: lagged`
   - Remove: `MetricsHistoryEntry` struct, `handle_system_metrics_history` function, `HistoryQueryParams` struct, `default_limit` function
   - Remove: `impl From<queries::SystemMetricsRow> for MetricsHistoryEntry`

3. In `proxy/server/router.rs`:
   - Remove the `/metrics/history` route
   - Remove `handle_system_metrics_history` from imports

4. Update tests:
   - `test_system_metrics_stream_emits_samples`: change from `event: sample` to `event: snapshot`, parse as `Vec<MetricSample>`
   - `test_system_metrics_stream_sample_models_round_trip`: same

**Steps:**
- [ ] Add VecDeque import and buffer seeding in `proxy/server/mod.rs`
- [ ] Define `row_into_sample` function with exact field mapping (see above)
- [ ] Rewrite main loop: read inference → build sample with inference → persist → push to buffer → broadcast Arc
- [ ] Rewrite `handle_system_metrics_stream` to emit `event: snapshot` with JSON array
- [ ] Add `samples.is_empty()` guard for shutdown sentinel
- [ ] Remove `inference_rx`, `inference_active`, `tokio::select!` from SSE handler
- [ ] Remove `MetricsHistoryEntry`, `handle_system_metrics_history`, `HistoryQueryParams`, `default_limit`
- [ ] Remove `/metrics/history` route and import from `router.rs`
- [ ] Update all tests for new types
- [ ] Run `cargo test --package tama-core proxy::server`
- [ ] Run `cargo test --package tama-core proxy::tama_handlers::system`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] **Commit Tasks 2 + 3 together** with message: "feat(core): unified MetricSample with inference stats + VecDeque snapshot buffer + SSE rewrite"

**Acceptance criteria:**
- [ ] Buffer seeded from SQLite on startup (450 rows via `row_into_sample`)
- [ ] Each tick: inference read → sample built → persisted → buffer updated → `Arc<[MetricSample]>` broadcast
- [ ] `row_into_sample` correctly maps all fields (i64→u64, Option<i64>→Option<u8>, Option<f64>→Option<f32>)
- [ ] SSE handler emits only `event: snapshot` with JSON array
- [ ] Empty Arc sentinel breaks the SSE loop (no empty snapshot sent to client)
- [ ] No `event: sample`, `event: inference`, or `event: lagged` events
- [ ] `/metrics/history` route removed
- [ ] All tests pass
- [ ] `cargo clippy --package tama-core -- -D warnings` passes
- [ ] Tasks 2 and 3 committed together as one atomic commit

**Context:**
The metrics task currently broadcasts a single `MetricSample` per tick. We need to: (a) add inference fields to `MetricSample`, (b) maintain an in-memory buffer of 450 samples using `VecDeque`, (c) seed the buffer from SQLite on startup, and (d) broadcast the full buffer each tick. The broadcast channel type changes from `Sender<MetricSample>` to `Sender<Vec<MetricSample>>`.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs`
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/server/mod.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`

**What to implement:**

1. In `gpu/system.rs` — `MetricSample` struct:
   - Add fields:
     ```rust
     pub tps: Option<f32>,
     pub prompt_tps: Option<f32>,
     pub cache_hit_pct: Option<f32>,
     pub spec_accept_pct: Option<f32>,
     pub spec_decoding_active: bool,
     pub inference_last_updated_ms: Option<i64>,
     ```
   - Add `#[serde(default)]` on the struct so missing fields deserialize as defaults

2. In `proxy/types.rs` — `ProxyState`:
   - Change `metrics_tx` type from `broadcast::Sender<crate::gpu::MetricSample>` to `broadcast::Sender<Vec<crate::gpu::MetricSample>>`
   - Change the `shutdown()` sentinel send to send an empty `Vec` instead of a dummy `MetricSample`

3. In `proxy/state.rs` — `ProxyState::new`:
   - Change `broadcast::channel(64)` to `broadcast::channel(2)` (old snapshots are immediately stale)

4. In `proxy/server/mod.rs` — metrics task in `ProxyServer::new`:
   - Add `use std::collections::VecDeque;`
   - Before the main loop, seed the buffer from SQLite:
     ```rust
     let mut history_buf: VecDeque<crate::gpu::MetricSample> = VecDeque::with_capacity(450);
     if let Some(seed_conn) = state.open_db() {
         if let Ok(rows) = crate::db::queries::get_recent_system_metrics(&seed_conn, 450) {
             for row in rows {
                 history_buf.push_back(row_into_sample(&row));
             }
         }
     }
     ```
     Where `row_into_sample` converts `SystemMetricsRow` to `MetricSample` (including inference fields from the row).
   - In the main loop, after building `sample`:
     - Push to buffer: `history_buf.push_back(sample);`
     - Trim: `while history_buf.len() > 450 { history_buf.pop_front(); }`
     - Read inference stats from watch channel:
       ```rust
       let inference = *inference_stats.borrow();
       ```
     - Include inference fields in the `sample`:
       ```rust
       tps: inference.as_ref().and_then(|i| i.tps),
       prompt_tps: inference.as_ref().and_then(|i| i.prompt_tps),
       cache_hit_pct: inference.as_ref().and_then(|i| i.cache_hit_pct),
       spec_accept_pct: inference.as_ref().and_then(|i| i.spec_accept_pct),
       spec_decoding_active: inference.map(|i| i.spec_decoding_active).unwrap_or(false),
       inference_last_updated_ms: inference.as_ref().and_then(|i| i.last_updated_ms),
       ```
     - Also include inference fields in the `SystemMetricsRow` for SQLite persistence
     - Change broadcast from `send(sample)` to `send(history_buf.iter().cloned().collect())`

   - Update all tests in the file that interact with `metrics_tx`:
     - `test_metrics_task_broadcasts_samples`: expect `Vec<MetricSample>` instead of `MetricSample`
     - `test_metric_sample_broadcast_populates_models_field`: same
     - Any test that checks the broadcast content

**Steps:**
- [ ] Add inference fields to `MetricSample` struct in `gpu/system.rs`
- [ ] Add `#[serde(default)]` on `MetricSample`
- [ ] Change `metrics_tx` type in `proxy/types.rs` to `Sender<Vec<MetricSample>>`
- [ ] Update `shutdown()` sentinel in `proxy/types.rs` to send empty Vec
- [ ] Change broadcast channel capacity to 2 in `proxy/state.rs`
- [ ] Add `VecDeque` import and buffer initialization in `proxy/server/mod.rs`
- [ ] Add SQLite seeding logic before the main loop
- [ ] Update the main loop to push to buffer, trim, read inference stats, and broadcast full buffer
- [ ] Update `SystemMetricsRow` usage to include inference fields
- [ ] Update all tests in `proxy/server/mod.rs`
- [ ] Run `cargo test --package tama-core proxy::server`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat(core): unified MetricSample with inference stats + VecDeque snapshot buffer"

**Acceptance criteria:**
- [ ] `MetricSample` has 6 new inference fields
- [ ] `metrics_tx` is `broadcast::Sender<Vec<MetricSample>>` with capacity 2
- [ ] Buffer is seeded from SQLite on startup (450 rows)
- [ ] Each tick broadcasts `Vec<MetricSample>` (full buffer clone)
- [ ] Inference stats from watch channel are included in each sample
- [ ] All existing tests pass (updated for new types)
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 3: Backend — SSE handler rewrite (snapshot events)

**Context:**
The SSE handler currently subscribes to `metrics_tx` (single samples) and `inference_stats` (watch channel), emitting `event: sample`, `event: inference`, and `event: lagged`. We replace this with a single subscription to `metrics_tx` (now `Vec<MetricSample>`) that emits `event: snapshot` with a JSON array. The separate `inference` event is eliminated — inference data is inside each sample.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/system.rs`

**What to implement:**

1. In `handle_system_metrics_stream`:
   - Remove the `inference_rx` watch channel subscription and all `inference` event handling
   - Remove the `inference_active` guard variable
   - Remove the `biased` select and `tokio::select!` — just a simple `rx.recv()` loop
   - On each received `Vec<MetricSample>`:
     - Serialize the entire vec to JSON
     - Yield `event: snapshot` with the JSON array as data
   - On `Lagged(n)`: instead of emitting a `lagged` event, just continue — the next snapshot will have full history
   - On `Closed`: break

   The handler becomes:
   ```rust
   pub async fn handle_system_metrics_stream(
       State(state): State<Arc<ProxyState>>,
   ) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
       let mut rx = state.metrics_tx.subscribe();
       let stream = async_stream::stream! {
           loop {
               match rx.recv().await {
                   Ok(samples) => {
                       match serde_json::to_string(&samples) {
                           Ok(data) => yield Ok(Event::default().event("snapshot").data(data)),
                           Err(e) => tracing::warn!("failed to serialize MetricSample vec: {}", e),
                       }
                   }
                   Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                       // Subscriber lagged — next snapshot will have full history, no action needed
                       continue;
                   }
                   Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
               }
           }
       };
       Sse::new(stream).keep_alive(KeepAlive::default())
   }
   ```

2. Remove the `MetricsHistoryEntry` struct and `handle_system_metrics_history` function (the history HTTP endpoint is no longer needed — the SSE snapshot replaces it).

3. Remove the `HistoryQueryParams` struct and `default_limit` function.

4. Update the router in `proxy/server/router.rs` — remove the `/metrics/history` route.

5. Update tests:
   - `test_system_metrics_stream_emits_samples`: change from looking for `event: sample` to `event: snapshot`, parse as `Vec<MetricSample>`
   - `test_system_metrics_stream_sample_models_round_trip`: same change

**Steps:**
- [ ] Rewrite `handle_system_metrics_stream` to emit `event: snapshot` with JSON array
- [ ] Remove `inference_rx` handling, `inference_active`, `tokio::select!`
- [ ] Remove `MetricsHistoryEntry`, `handle_system_metrics_history`, `HistoryQueryParams`, `default_limit`
- [ ] Remove `/metrics/history` route from `proxy/server/router.rs`
- [ ] Update SSE tests to expect `event: snapshot` with `Vec<MetricSample>`
- [ ] Run `cargo test --package tama-core proxy::tama_handlers::system`
- [ ] Run `cargo test --package tama-core proxy::server::tests::test_system_metrics_stream`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat(core): rewrite SSE handler to emit snapshot events, remove history endpoint"

**Acceptance criteria:**
- [ ] SSE handler emits only `event: snapshot` with JSON array of `Vec<MetricSample>`
- [ ] No `event: sample`, `event: inference`, or `event: lagged` events
- [ ] No `inference_rx` watch channel polling in SSE handler
- [ ] `handle_system_metrics_history` and related types removed
- [ ] `/metrics/history` route removed from router
- [ ] All SSE tests pass with new event format
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 4: Frontend — types + dashboard simplification

**Context:**
The frontend currently has two data paths: (1) HTTP GET for history + SSE `sample` events for system metrics, and (2) SSE `inference` events for inference stats. With the snapshot stream, both paths merge into one: SSE `snapshot` events containing `Vec<MetricSample>` with all fields. This eliminates backfill, merge, lag detection, and visibility change handlers.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/metrics.rs`
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs`

**What to implement:**

1. In `dashboard/metrics.rs`:
   - Add inference fields to `MetricSample`:
     ```rust
     pub tps: Option<f32>,
     pub prompt_tps: Option<f32>,
     pub cache_hit_pct: Option<f32>,
     pub spec_accept_pct: Option<f32>,
     pub spec_decoding_active: bool,
     pub inference_last_updated_ms: Option<i64>,
     ```
   - Remove: `MetricsHistoryEntry` struct and its `From` impl
   - Remove: `InferenceStats` struct
   - Remove: `is_stale` function
   - Remove: `format_stale_time` function
   - Remove: `merge_samples` function
   - Remove: `backfill_metrics` async function
   - Keep: `MetricSample`, `VramInfo`, `ModelStatus`, `format_number`, `active_models`, `inactive_models`, `model_display_name`, `model_sort_key`

2. In `dashboard/mod.rs`:
   - Remove: `fetch_failed` signal usage for history fetch (keep for SSE errors)
   - Remove: `last_backfill` signal
   - Remove: `reconnect_pending` signal
   - Remove: Initial `GET /metrics/history` fetch `spawn_local` block
   - Remove: `visibilitychange` Effect (the one with `Reflect::get(&doc, &"hidden"...`)
   - Remove: `inference_history` signal
   - Remove: `INFERENCE_MAX_LEN` constant

   - Rewrite the SSE Effect:
     - Keep `connect_trigger` and `fetch_failed` signals
     - Replace `sample` event handler with `snapshot` handler:
       ```rust
       let on_snapshot = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
           if let Some(data_str) = evt.data().as_string() {
               if let Ok(samples) = serde_json::from_str::<Vec<MetricSample>>(&data_str) {
                   fetch_failed.set(false);
                   history.set(samples);  // Replace entire buffer
               }
           }
       });
       es.add_event_listener_with_callback("snapshot", on_snapshot.as_ref().unchecked_ref());
       on_snapshot.forget();
       ```
     - Remove `lagged` event handler
     - Remove `inference` event handler
     - Simplify error handler to just `fetch_failed.set(true)`
     - **CRITICAL: Keep the `on_cleanup` closure that calls `es.close()`** — without it, reconnecting via `connect_trigger` leaks EventSource objects
     - Note on sparkline semantics: inference stats are now sampled every 2s (step-function) rather than sparse observations. Sparklines will show repeated identical values between backend updates. This is acceptable — the 2s resolution is sufficient for dashboard display. If distinct-observation sparklines are desired later, filter samples where `inference_last_updated_ms` hasn't changed since the previous sample.

   - Update the inference stats display section:
     - Instead of reading from `inference_history.get()`, read from `history.get().last()`
     - Extract `tps`, `prompt_tps`, `cache_hit_pct`, `spec_accept_pct` from the latest `MetricSample`
     - For sparkline data, extract from all samples in history (use `unwrap_or(0.0)` for None values)
     - Replace `is_stale(latest.last_updated_ms)` with a check on `inference_last_updated_ms`
     - Replace `format_stale_time` usage with a simple inline format or keep a minimal version

   - Update the `manual_refresh` callback — remove `reconnect_pending.set(false)`

**Steps:**
- [ ] Add inference fields to frontend `MetricSample` in `metrics.rs`
- [ ] Remove `MetricsHistoryEntry`, `InferenceStats`, `is_stale`, `format_stale_time`, `merge_samples`, `backfill_metrics` from `metrics.rs`
- [ ] Remove initial history fetch, `last_backfill`, `reconnect_pending`, `visibilitychange` handler, `inference_history` from `mod.rs`
- [ ] Rewrite SSE Effect to handle `event: snapshot` → `history.set(samples)`
- [ ] Remove `lagged` and `inference` event handlers
- [ ] Update inference stats display to read from `history.get().last()`
- [ ] Update sparkline data extraction for inference stats from main history buffer
- [ ] Run `cargo check --package tama-web`
  - Did it compile? If not, fix type errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Run `cargo build --package tama-web`
- [ ] Commit with message: "feat(web): simplify dashboard to use snapshot SSE events, remove backfill logic"

**Acceptance criteria:**
- [ ] Frontend `MetricSample` has inference fields matching backend
- [ ] No `MetricsHistoryEntry`, `InferenceStats`, `backfill_metrics`, `merge_samples` in frontend
- [ ] SSE handler listens only for `event: snapshot`, replaces `history` signal with parsed `Vec<MetricSample>`
- [ ] No `GET /metrics/history` fetch on mount
- [ ] No `visibilitychange` handler, `lagged` handler, `inference` handler
- [ ] Inference stats display reads from `history.get().last()`
- [ ] `cargo clippy --package tama-web -- -D warnings` passes
- [ ] `cargo build --package tama-web` passes

---

### Task 5: Verification — workspace tests + cleanup

**Context:**
After all individual changes, run the full workspace test suite and clippy to catch any cross-crate issues. Also verify that no dead code remains (e.g., unused imports from removed functions).

**Files:**
- May modify: any file with clippy warnings or test failures

**What to implement:**
- Run the full workspace check suite
- Fix any remaining issues
- Verify no references to removed types/functions remain

**Steps:**
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix all warnings and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Search for any remaining references to removed types:
  - `grep -r "MetricsHistoryEntry" crates/` — should find nothing in frontend, only backend if kept
  - `grep -r "InferenceStats" crates/tama-web/` — should find nothing
  - `grep -r "backfill_metrics" crates/` — should find nothing
  - `grep -r "merge_samples" crates/` — should find nothing
  - `grep -r "inference_history" crates/tama-web/` — should find nothing
- [ ] Commit with message: "chore: workspace verification — clippy clean, all tests pass"

**Acceptance criteria:**
- [ ] `cargo check --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] No references to removed types in frontend code
- [ ] No dead code warnings

---

## Summary

| Task | Scope | Key Files |
|------|-------|-----------|
| 1 | Database schema + queries | `migrations.rs`, `metrics_queries.rs` |
| 2+3 | Backend: MetricSample + buffer + SSE rewrite (atomic) | `system.rs`, `types.rs`, `state.rs`, `server/mod.rs`, `tama_handlers/system.rs`, `router.rs` |
| 4 | Frontend types + dashboard | `dashboard/metrics.rs`, `dashboard/mod.rs` |
| 5 | Workspace verification | All crates |

## Deleted code (after plan completion)

- `backfill_metrics` function (frontend)
- `merge_samples` function (frontend)
- `MetricsHistoryEntry` struct (frontend)
- `InferenceStats` struct (frontend)
- `is_stale` / `format_stale_time` functions (frontend)
- `handle_system_metrics_history` endpoint (backend)
- `event: sample`, `event: inference`, `event: lagged` SSE events
- `/metrics/history` HTTP route
- `last_backfill`, `reconnect_pending` signals (frontend)
- `visibilitychange` handler (frontend)
- `inference_history` signal (frontend)
