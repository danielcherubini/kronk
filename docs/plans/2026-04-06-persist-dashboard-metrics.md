# Persist Dashboard Metrics to SQLite + Stream via SSE — Plan

**Goal:** Replace the dashboard's volatile in-memory metrics buffer with a SQLite-backed time series, served live to the UI over a Server-Sent Events stream, with automatic pruning so the database stays bounded.

**Architecture:** The existing 5-second background metrics refresh task (`ProxyServer::new` in `proxy/server/mod.rs`) is dropped to a 2-second cadence and extended to (a) write each sample to a new `system_metrics_history` SQLite table, (b) prune rows older than `proxy.metrics_retention_secs` in the same transaction, and (c) broadcast the sample over a `tokio::sync::broadcast` channel held in `ProxyState`. A new SSE endpoint `GET /koji/v1/system/metrics/stream` subscribes to that channel and forwards samples to clients. The Leptos dashboard replaces its `setInterval` polling with a `web_sys::EventSource` and appends streamed samples to its existing `RwSignal<Vec<...>>` ring buffer. Decisions confirmed by user: configurable retention via `proxy.metrics_retention_secs` (default 24h), 2-second cadence, inline pruning, no historical backfill in the SSE response, full schema (`SystemMetrics` + `models_loaded`), silent no-op when `db_dir` is `None`, and `/koji/v1/system/health` left untouched.

**Tech Stack:** Rust, axum (SSE via `axum::response::sse`), `async-stream` 0.3 (already in `koji-core/Cargo.toml`), `rusqlite` (existing `db` module), `tokio::sync::broadcast`, Leptos 0.7 CSR, `web-sys::EventSource`.

---

## Task 1: Add `metrics_retention_secs` to `ProxyConfig`

**Context:**
The plan requires a user-tunable retention window for stored metric samples. The proxy already has a `ProxyConfig` struct in `crates/koji-core/src/config/types.rs` that uses `serde` with `#[serde(default = "...")]` helpers (e.g. `idle_timeout_secs`, `circuit_breaker_threshold`). We mirror that pattern. Default is 86400 seconds (24 hours). This is the *only* config change needed for the entire feature; it must be in place before Task 4 (the metrics task) reads it.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs`
- Modify: `config/koji.toml` (or `config/koji.toml.example` — whichever is the canonical template; verify with `ls config/` first)

**What to implement:**

1. In `crates/koji-core/src/config/types.rs`, add a new field to `ProxyConfig`:
   ```rust
   #[serde(default = "default_metrics_retention")]
   pub metrics_retention_secs: u64,
   ```
   Place it after `circuit_breaker_cooldown_seconds`.

2. Add the default helper near the other `default_*` functions (around the existing `default_circuit_breaker_threshold` at line ~215):
   ```rust
   fn default_metrics_retention() -> u64 {
       86_400
   }
   ```

3. Update `impl Default for ProxyConfig` to include `metrics_retention_secs: default_metrics_retention()`.

4. In the canonical config template under `config/`, add a documented entry under `[proxy]`:
   ```toml
   # How long to keep system-metric samples in the SQLite history table.
   # Older rows are pruned on every metrics tick. Default: 86400 (24 hours).
   metrics_retention_secs = 86400
   ```

**Steps:**
- [ ] Run `ls config/` to identify the canonical config template file.
- [ ] Write a failing unit test in `crates/koji-core/src/config/types.rs` (in the existing `#[cfg(test)] mod tests` block, or add one if absent) named `test_proxy_config_default_metrics_retention` that constructs `ProxyConfig::default()` and asserts `metrics_retention_secs == 86_400`.
- [ ] Run `cargo test --package koji-core test_proxy_config_default_metrics_retention`
  - Did it fail with a compile error about the missing field? If it passed unexpectedly, stop and investigate.
- [ ] Add the field, default helper, and `Default` impl entry as described above.
- [ ] Add a second test `test_proxy_config_deserializes_metrics_retention` that round-trips `[proxy]\nmetrics_retention_secs = 3600\n` through `toml::from_str::<ProxyConfig>` and asserts the value is 3600.
- [ ] Run `cargo test --package koji-core --lib config::types::tests`
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Add the documented entry to the canonical config template.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: `feat(config): add proxy.metrics_retention_secs with 24h default`

**Acceptance criteria:**
- [ ] `ProxyConfig` has a `metrics_retention_secs: u64` field that defaults to 86400.
- [ ] The field deserializes from TOML and falls back to the default when omitted.
- [ ] The canonical config template documents the new key.
- [ ] All workspace tests still pass.

---

## Task DB migration v4 — `system_metrics_history` table and queries

**Context:**
Koji already uses a SQLite database at `<config_dir>/koji.db` with a `PRAGMA user_version`-driven migration system in `crates/koji-core/src/db/migrations.rs` (currently at `LATEST_VERSION = 3`). All write/read helpers live in `crates/koji-core/src/db/queries.rs` and take a plain `&Connection`. We add a new migration (v4) that creates `system_metrics_history`, plus typed helpers to insert (with inline pruning) and read recent rows. This task is independently committable: nothing else in the codebase references the new table or helpers yet. Time is stored as `INTEGER` unix milliseconds for sortability and simple range queries.

**Files:**
- Modify: `crates/koji-core/src/db/migrations.rs`
- Modify: `crates/koji-core/src/db/queries.rs`
- Modify: `crates/koji-core/src/db/mod.rs` (test only)

**What to implement:**

1. **Migration v4** in `crates/koji-core/src/db/migrations.rs`:
   - Bump `pub const LATEST_VERSION: i32 = 3;` to `4`.
   - Append a new tuple to the `migrations` slice:
     ```rust
     (
         4,
         r#"
             CREATE TABLE IF NOT EXISTS system_metrics_history (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts_unix_ms          INTEGER NOT NULL,
                 cpu_usage_pct       REAL    NOT NULL,
                 ram_used_mib        INTEGER NOT NULL,
                 ram_total_mib       INTEGER NOT NULL,
                 gpu_utilization_pct INTEGER,
                 vram_used_mib       INTEGER,
                 vram_total_mib      INTEGER,
                 models_loaded       INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_system_metrics_ts
                 ON system_metrics_history(ts_unix_ms);
             "#,
     ),
     ```

2. **New record type and queries** in `crates/koji-core/src/db/queries.rs`. Add at the end of the "Record types" section (after `BackendInstallationRecord`):
   ```rust
   /// One sample of system-level metrics, persisted in `system_metrics_history`.
   #[derive(Debug, Clone)]
   pub struct SystemMetricsRow {
       pub ts_unix_ms: i64,
       pub cpu_usage_pct: f32,
       pub ram_used_mib: i64,
       pub ram_total_mib: i64,
       pub gpu_utilization_pct: Option<i64>,
       pub vram_used_mib: Option<i64>,
       pub vram_total_mib: Option<i64>,
       pub models_loaded: i64,
   }
   ```

   Add a new section "System metrics history query functions" with three functions:

   ```rust
   /// Insert one sample and prune anything older than `cutoff_ms` in a single
   /// transaction. Both operations succeed or fail together so a crash never
   /// leaves the table half-pruned.
   pub fn insert_system_metric(
       conn: &Connection,
       row: &SystemMetricsRow,
       cutoff_ms: i64,
   ) -> Result<()> {
       let tx = conn.unchecked_transaction()?;
       tx.execute(
           "INSERT INTO system_metrics_history
                (ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
                 gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
           (
               row.ts_unix_ms,
               row.cpu_usage_pct as f64,
               row.ram_used_mib,
               row.ram_total_mib,
               row.gpu_utilization_pct,
               row.vram_used_mib,
               row.vram_total_mib,
               row.models_loaded,
           ),
       )?;
       tx.execute(
           "DELETE FROM system_metrics_history WHERE ts_unix_ms < ?1",
           [cutoff_ms],
       )?;
       tx.commit()?;
       Ok(())
   }

   /// Fetch all samples newer than `since_ms` (exclusive), oldest-first.
   pub fn get_system_metrics_since(
       conn: &Connection,
       since_ms: i64,
   ) -> Result<Vec<SystemMetricsRow>> { /* SELECT ... WHERE ts_unix_ms > since_ms ORDER BY ts_unix_ms ASC */ }

   /// Fetch the most recent `limit` samples, oldest-first.
   pub fn get_recent_system_metrics(
       conn: &Connection,
       limit: i64,
   ) -> Result<Vec<SystemMetricsRow>> {
       // SELECT ... ORDER BY ts_unix_ms DESC LIMIT ?1, then reverse in Rust.
   }
   ```

   The two `get_*` functions are added even though Task 5 (per user decision) won't use them for backfill — they're cheap to write, fully testable, and useful for ad-hoc inspection / future tooling. **If the reviewer pushes back on YAGNI, only `get_recent_system_metrics` may be deleted; `insert_system_metric` is required by Task 4.**

3. **Migration test** in `crates/koji-core/src/db/mod.rs::tests`:
   ```rust
   #[test]
   fn test_migration_v4_creates_system_metrics_history() {
       let OpenResult { conn, .. } = open_in_memory().unwrap();
       let count: i64 = conn.query_row(
           "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='system_metrics_history'",
           [], |row| row.get(0)).unwrap();
       assert_eq!(count, 1);
       let idx_count: i64 = conn.query_row(
           "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_system_metrics_ts'",
           [], |row| row.get(0)).unwrap();
       assert_eq!(idx_count, 1);
   }
   ```

   Also update `test_user_version_updated` — it asserts `version == migrations::LATEST_VERSION`, which will continue to pass automatically once `LATEST_VERSION` is bumped to 4. No code change needed there, but verify it still passes.

**What NOT to change:**
- Do not touch any existing migration tuple — version numbers are immutable history.
- Do not modify `LATEST_VERSION` to a non-monotonic value.

**Steps:**
- [ ] Write a failing test `test_migration_v4_creates_system_metrics_history` in `crates/koji-core/src/db/mod.rs::tests`.
- [ ] Run `cargo test --package koji-core test_migration_v4_creates_system_metrics_history`
  - Did it fail because the table doesn't exist? If it passed unexpectedly, stop and investigate.
- [ ] Bump `LATEST_VERSION` to 4 and add the migration tuple in `crates/koji-core/src/db/migrations.rs`.
- [ ] Re-run `cargo test --package koji-core test_migration_v4_creates_system_metrics_history`
  - Did it pass? If not, fix and re-run.
- [ ] Write failing tests in `crates/koji-core/src/db/queries.rs::tests`:
  - `test_insert_and_get_recent_system_metrics`: insert 3 rows with distinct `ts_unix_ms`, call `get_recent_system_metrics(conn, 10)`, assert ordering oldest-first and correct field values.
  - `test_insert_system_metric_prunes_old_rows`: insert one row at `ts = 1000`, then insert a second row at `ts = 5000` with `cutoff_ms = 4000`, then assert only one row remains (`SELECT COUNT(*)`).
  - `test_get_system_metrics_since`: insert rows at `ts = 1000, 2000, 3000`, call `get_system_metrics_since(conn, 1500)`, assert exactly 2 rows returned in ascending order.
- [ ] Run `cargo test --package koji-core --lib db::queries::tests`
  - Did all three tests fail to compile (missing functions)? If they passed, stop and investigate.
- [ ] Add `SystemMetricsRow` and the three helper functions to `crates/koji-core/src/db/queries.rs`.
- [ ] Re-run `cargo test --package koji-core --lib db::queries::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: `feat(db): add system_metrics_history table (migration v4) with insert+prune helpers`

**Acceptance criteria:**
- [ ] `LATEST_VERSION` is 4 and a fresh DB reports `user_version = 4`.
- [ ] `system_metrics_history` table and `idx_system_metrics_ts` index exist after migration.
- [ ] `insert_system_metric` inserts a row and deletes anything older than `cutoff_ms` atomically.
- [ ] `get_system_metrics_since` and `get_recent_system_metrics` return rows oldest-first.
- [ ] No existing migration tuple was modified.

---

## Task Add `MetricSample` type and broadcast channel to `ProxyState`

**Context:**
We need a single in-process pub/sub channel so the metrics task (one producer) can fan out samples to N concurrent SSE subscribers (zero or more consumers). `tokio::sync::broadcast` is the right primitive: it handles slow consumers via `RecvError::Lagged` and tolerates zero subscribers (the producer's `send` just returns `Err(SendError)` which we ignore). We also introduce a new `MetricSample` struct that bundles everything the dashboard needs into a single serializable record. This struct is the wire format for the SSE stream and the in-memory broadcast payload.

**Files:**
- Modify: `crates/koji-core/src/gpu.rs` (add `MetricSample` near `SystemMetrics`)
- Modify: `crates/koji-core/src/proxy/types.rs` (add `metrics_tx` field to `ProxyState`)
- Modify: `crates/koji-core/src/proxy/state.rs` (initialize `metrics_tx` in `ProxyState::new`)

**What to implement:**

1. **`MetricSample` struct** in `crates/koji-core/src/gpu.rs`, immediately below `SystemMetrics`:
   ```rust
   /// A timestamped snapshot of system + proxy metrics, suitable for persistence
   /// in `system_metrics_history` and broadcast over the SSE stream.
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct MetricSample {
       pub ts_unix_ms: i64,
       pub cpu_usage_pct: f32,
       pub ram_used_mib: u64,
       pub ram_total_mib: u64,
       pub gpu_utilization_pct: Option<u8>,
       pub vram: Option<VramInfo>,
       pub models_loaded: u64,
   }
   ```

2. **Add field to `ProxyState`** in `crates/koji-core/src/proxy/types.rs` (around line 134):
   ```rust
   pub metrics_tx: tokio::sync::broadcast::Sender<crate::gpu::MetricSample>,
   ```
   Add it as the last field. Do not derive any new traits — `broadcast::Sender` is `Clone` and `Send + Sync`, which is what `#[derive(Clone)]` on `ProxyState` already requires.

3. **Initialize in `ProxyState::new`** in `crates/koji-core/src/proxy/state.rs`. Add inside the constructor (before the struct literal):
   ```rust
   let (metrics_tx, _) = tokio::sync::broadcast::channel(64);
   ```
   Then add `metrics_tx,` to the struct literal. Capacity 64 ≈ 2 minutes of 2s ticks of headroom for slow subscribers.

**What NOT to change:**
- Do not touch the existing `system_metrics: Arc<RwLock<SystemMetrics>>` field — it's still used by `/koji/v1/system/health`.
- Do not change any other field in `ProxyState` or `MetricSample`'s field order without good reason; downstream serde consumers depend on the JSON field names.

**Steps:**
- [ ] Write a failing unit test `test_proxy_state_new_creates_metrics_channel` in `crates/koji-core/src/proxy/state.rs` (or wherever `ProxyState` tests live — search with `rg "fn test_" crates/koji-core/src/proxy/state.rs`; if no test module exists, add one). The test constructs a `ProxyState`, calls `state.metrics_tx.subscribe()`, and asserts the receiver count is now 1.
- [ ] Run `cargo test --package koji-core test_proxy_state_new_creates_metrics_channel`
  - Did it fail because `metrics_tx` doesn't exist? If it passed unexpectedly, stop and investigate.
- [ ] Add `MetricSample` to `crates/koji-core/src/gpu.rs`.
- [ ] Add the `metrics_tx` field to `ProxyState`.
- [ ] Initialize the channel in `ProxyState::new`.
- [ ] Re-run `cargo test --package koji-core test_proxy_state_new_creates_metrics_channel`
  - Did it pass? If not, fix and re-run.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix all call sites and re-run. (No external callers should construct `ProxyState` literals; only `ProxyState::new` is used. Verify with `rg "ProxyState \{" crates/`.)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: `feat(proxy): add MetricSample type and broadcast channel to ProxyState`

**Acceptance criteria:**
- [ ] `MetricSample` exists in `gpu.rs` and is `Clone + Serialize + Deserialize`.
- [ ] `ProxyState` has a `metrics_tx: broadcast::Sender<MetricSample>` field.
- [ ] `ProxyState::new` creates the channel with capacity 64.
- [ ] `ProxyState` still derives `Clone` and the workspace builds.

---

## Task Drop metrics interval to 2s and persist + broadcast each sample

**Context:**
`ProxyServer::new` in `crates/koji-core/src/proxy/server/mod.rs` currently spawns a Tokio task that calls `crate::gpu::collect_system_metrics_with` every 5 seconds and writes the result to `state.system_metrics`. We extend this task to:
1. Run every **2 seconds** instead of 5.
2. Read `state.models.read().await.len()` to capture `models_loaded`.
3. Build a `MetricSample` and **persist it best-effort** to SQLite via `db::queries::insert_system_metric`, computing the cutoff from `proxy.metrics_retention_secs`.
4. **Broadcast** the sample over `state.metrics_tx`.

The DB write is wrapped in `tokio::task::spawn_blocking` because rusqlite is synchronous and we don't want to stall the metrics loop on a slow flush. Persistence and broadcast are both best-effort: failures (no DB, no subscribers, lagged subscribers) are logged at `warn!` level but never propagate. The existing `state.system_metrics.write()` update stays — it's still the source for the `/koji/v1/system/health` snapshot endpoint, which we are explicitly leaving untouched (per user decision).

The current task captures `metrics_arc = Arc::clone(&state.system_metrics)`. We refactor to capture `state_clone = Arc::clone(&state)` so we can read `models`, `config`, `metrics_tx`, and `open_db()` from the same handle. This is consistent with the existing `start_idle_timeout_checker` which already takes the full `Arc<ProxyState>`.

**Files:**
- Modify: `crates/koji-core/src/proxy/server/mod.rs`

**What to implement:**

Replace the existing `metrics_handle` block (currently lines ~25–46) with the following:

```rust
// Spawn background task to refresh system metrics every 2s.
// Each tick: collect metrics, update the cached snapshot, persist to SQLite
// (best-effort, with inline pruning), and broadcast to SSE subscribers.
let metrics_state = Arc::clone(&state);
let metrics_handle = tokio::spawn(async move {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut sys = sysinfo::System::new();
    loop {
        // Collect metrics on a blocking thread.
        let (snapshot, returned_sys) = tokio::task::spawn_blocking(move || {
            let snapshot = crate::gpu::collect_system_metrics_with(&mut sys);
            (snapshot, sys)
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("system metrics collection panicked: {}", e);
            (crate::gpu::SystemMetrics::default(), sysinfo::System::new())
        });
        sys = returned_sys;

        // Update the cached snapshot read by /koji/v1/system/health.
        *metrics_state.system_metrics.write().await = snapshot.clone();

        // Build a timestamped MetricSample.
        let ts_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let models_loaded = metrics_state.models.read().await.len() as u64;
        let sample = crate::gpu::MetricSample {
            ts_unix_ms,
            cpu_usage_pct: snapshot.cpu_usage_pct,
            ram_used_mib: snapshot.ram_used_mib,
            ram_total_mib: snapshot.ram_total_mib,
            gpu_utilization_pct: snapshot.gpu_utilization_pct,
            vram: snapshot.vram.clone(),
            models_loaded,
        };

        // Persist to SQLite (best-effort). Read retention from config.
        let retention_secs = metrics_state.config.read().await.proxy.metrics_retention_secs;
        if let Some(conn) = metrics_state.open_db() {
            let row = crate::db::queries::SystemMetricsRow {
                ts_unix_ms: sample.ts_unix_ms,
                cpu_usage_pct: sample.cpu_usage_pct,
                ram_used_mib: sample.ram_used_mib as i64,
                ram_total_mib: sample.ram_total_mib as i64,
                gpu_utilization_pct: sample.gpu_utilization_pct.map(|v| v as i64),
                vram_used_mib: sample.vram.as_ref().map(|v| v.used_mib as i64),
                vram_total_mib: sample.vram.as_ref().map(|v| v.total_mib as i64),
                models_loaded: sample.models_loaded as i64,
            };
            let cutoff_ms = sample.ts_unix_ms - (retention_secs as i64 * 1000);
            // Run the blocking SQLite call off the runtime.
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(e) = crate::db::queries::insert_system_metric(&conn, &row, cutoff_ms) {
                    tracing::warn!("failed to persist system metric: {}", e);
                }
            })
            .await;
        }

        // Broadcast to any live SSE subscribers. SendError just means there are
        // no subscribers; that is the normal idle case.
        let _ = metrics_state.metrics_tx.send(sample);

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
});
```

**What NOT to change:**
- Do not touch `cleanup_stale_processes`, `start_idle_timeout_checker`, or any other method on `ProxyServer`.
- Do not change the order of operations in `ProxyServer::new` — `cleanup_stale_processes` must run first, then `start_idle_timeout_checker`, then the metrics task.
- Do not remove the `unwrap_or_else` panic guard around `spawn_blocking`.

**Steps:**
- [ ] Write a failing async test in `crates/koji-core/src/proxy/server/mod.rs::tests` named `test_metrics_task_persists_to_db`. The test:
  1. Creates a `tempfile::tempdir()`.
  2. Builds a `Config::default()` and a `ProxyState::new(config, Some(tmpdir.path().to_path_buf()))`.
  3. Constructs a `ProxyServer::new(state.clone())`.
  4. Sleeps for 2.5 seconds (`tokio::time::sleep`).
  5. Opens the DB via `state.open_db().unwrap()` and asserts `SELECT COUNT(*) FROM system_metrics_history >= 1`.
- [ ] Write a second failing async test `test_metrics_task_broadcasts_samples`:
  1. Same setup.
  2. Subscribe to `state.metrics_tx.subscribe()` BEFORE constructing `ProxyServer`.
  3. Construct the server.
  4. `tokio::time::timeout(Duration::from_secs(4), rx.recv()).await` and assert `Ok(Ok(_))`.
- [ ] Run `cargo test --package koji-core test_metrics_task_persists_to_db test_metrics_task_broadcasts_samples`
  - Did both fail? (One because the table is empty/unused, one because no broadcast happens.) If they passed, stop and investigate.
- [ ] Apply the refactor described above to `ProxyServer::new`.
- [ ] Re-run `cargo test --package koji-core test_metrics_task_persists_to_db test_metrics_task_broadcasts_samples`
  - Did both pass? If not, debug and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: `feat(proxy): persist + broadcast system metrics every 2s`

**Acceptance criteria:**
- [ ] Metrics task ticks every 2 seconds (verified by the >=1-row count after 2.5s).
- [ ] Each tick writes one row to `system_metrics_history` and prunes rows older than `metrics_retention_secs`.
- [ ] Each tick broadcasts a `MetricSample` over `metrics_tx`.
- [ ] DB write failures are logged at `warn!` and never panic the loop.
- [ ] `state.system_metrics` cached snapshot is still updated (so `/koji/v1/system/health` keeps working).
- [ ] When `db_dir` is `None`, the loop still ticks and broadcasts (silent no-op for persistence).

---

## Task New SSE endpoint `GET /koji/v1/system/metrics/stream`

**Context:**
We expose the broadcast channel to web clients via Server-Sent Events. The dashboard will connect once on mount and consume samples for as long as the page is open. Per user decision in the planning conversation: **no historical backfill** — the stream emits only live samples from the moment the client subscribes. The handler subscribes to `state.metrics_tx`, then uses `async_stream::stream!` to forward each received `MetricSample` as an SSE `event: "sample"` with the JSON-serialized payload as `data`. On `RecvError::Lagged(n)` we emit a `event: "lagged"` data event with the count and continue. On `RecvError::Closed` we end the stream cleanly.

This mirrors the existing `handle_pull_job_stream` in `crates/koji-core/src/proxy/koji_handlers.rs:736` (which uses `Sse::new(stream).keep_alive(KeepAlive::default())`), except we use `async_stream::stream!` instead of `futures::stream::unfold` because we have a single-phase loop with no per-iteration state.

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs` (new handler)
- Modify: `crates/koji-core/src/proxy/server/router.rs` (route registration)

**What to implement:**

1. **Handler** in `crates/koji-core/src/proxy/koji_handlers.rs`. Add at the end of the file (or just below `handle_koji_system_health`):

   ```rust
   /// Stream live system metrics samples as SSE events.
   ///
   /// Subscribes to the `metrics_tx` broadcast channel in `ProxyState`. Each
   /// sample emitted by the metrics task (every 2s) is forwarded as an
   /// `event: "sample"` SSE event with a JSON-serialized `MetricSample` body.
   /// On subscriber lag, emits an `event: "lagged"` event with `{"missed": N}`
   /// and continues. On channel close, the stream ends.
   ///
   /// No historical backfill — the stream begins from the next live sample.
   ///
   /// Registered as `GET /koji/v1/system/metrics/stream`.
   pub async fn handle_system_metrics_stream(
       State(state): State<Arc<ProxyState>>,
   ) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
       let mut rx = state.metrics_tx.subscribe();
       let stream = async_stream::stream! {
           loop {
               match rx.recv().await {
                   Ok(sample) => {
                       let data = serde_json::to_string(&sample).unwrap_or_default();
                       yield Ok(Event::default().event("sample").data(data));
                   }
                   Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                       let data = format!("{{\"missed\":{}}}", n);
                       yield Ok(Event::default().event("lagged").data(data));
                   }
                   Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                       break;
                   }
               }
           }
       };
       Sse::new(stream).keep_alive(KeepAlive::default())
   }
   ```

   **Imports to verify at the top of the file:** `Sse`, `Event`, `KeepAlive`, `Stream`, `Infallible`, `Arc`, `State`. The first three already exist (line 7 imports `sse::{Event, KeepAlive, Sse}`). `Stream` and `Infallible` are imported for the existing pull-job handler. `async_stream` is a new direct usage — no `use` needed since we use the macro path `async_stream::stream!`.

2. **Route registration** in `crates/koji-core/src/proxy/server/router.rs`:
   - Add `handle_system_metrics_stream` to the `use crate::proxy::koji_handlers::{...};` list (the import block at line 12).
   - Add the route to `build_router` immediately after the existing `/koji/v1/system/health` line (line 48):
     ```rust
     .route("/koji/v1/system/metrics/stream", get(handle_system_metrics_stream))
     ```

**What NOT to change:**
- Do not modify `handle_koji_system_health` or its response struct (per user decision).
- Do not add query parameters for backfill (per user decision: "i dont care about backfill").
- Do not modify any other route or handler.

**Steps:**
- [ ] Write a failing async integration test in `crates/koji-core/src/proxy/server/mod.rs::tests` named `test_system_metrics_stream_emits_samples`. The test:
  1. Builds a `Config::default()` and `ProxyState::new(config, Some(tmpdir.path().to_path_buf()))`.
  2. Builds a `ProxyServer::new(state.clone())` and starts the router on `127.0.0.1:0`.
  3. Uses `reqwest::Client` to `GET /koji/v1/system/metrics/stream` with a 5-second timeout.
  4. Reads the response body as a stream (`response.bytes_stream()`) and waits for the first chunk that contains `event: sample`.
  5. Asserts the chunk parses an SSE `data:` line into a `MetricSample` (use `serde_json::from_str`).
- [ ] Run `cargo test --package koji-core test_system_metrics_stream_emits_samples`
  - Did it fail because the route returns 404? If it passed, stop and investigate.
- [ ] Add the handler to `crates/koji-core/src/proxy/koji_handlers.rs`.
- [ ] Wire the route in `crates/koji-core/src/proxy/server/router.rs`.
- [ ] Re-run `cargo test --package koji-core test_system_metrics_stream_emits_samples`
  - Did it pass? If it times out, double-check that Task 4 is committed and the metrics task is actually running.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: `feat(api): add GET /koji/v1/system/metrics/stream SSE endpoint`

**Acceptance criteria:**
- [ ] `GET /koji/v1/system/metrics/stream` returns `text/event-stream` and emits `sample` events.
- [ ] Each `sample` event's `data` field is a JSON `MetricSample`.
- [ ] The handler subscribes to `state.metrics_tx` (no polling, no DB reads).
- [ ] `RecvError::Lagged` is handled by emitting a `lagged` event and continuing.
- [ ] `/koji/v1/system/health` still works exactly as before.

---

## Task Document the new endpoint in OpenAPI

**Context:**
The Koji management API is documented in `docs/openapi/koji-api.yaml`. The existing `/koji/v1/system/health` entry lives around line 301. We add a peer entry for `/koji/v1/system/metrics/stream` and a `MetricSample` schema. This task is independently committable: it touches only docs and has no Rust code dependencies (the schema is hand-written to match Task 3's struct).

**Files:**
- Modify: `docs/openapi/koji-api.yaml`

**What to implement:**

1. Under `paths:`, immediately after the `/koji/v1/system/health` block (around line 337), add:

   ```yaml
   /koji/v1/system/metrics/stream:
     get:
       operationId: streamSystemMetrics
       summary: Live system metrics stream (SSE)
       description: |
         Streams live `MetricSample` events as Server-Sent Events. A new sample
         is emitted every ~2 seconds by the proxy's background metrics task.

         Samples are persisted to a SQLite history table (pruned to the
         configured retention window via `proxy.metrics_retention_secs`), but
         this endpoint only emits live samples — there is no historical
         backfill on connect.

         Event types:
           - `sample` — JSON-serialized `MetricSample`.
           - `lagged` — emitted when the subscriber falls behind; payload
             `{"missed": N}`. The stream continues after this event.
       tags: [system]
       responses:
         "200":
           description: SSE stream of metric samples
           content:
             text/event-stream:
               schema:
                 type: string
                 description: |
                   SSE-framed stream. Each `sample` event's `data` field is a
                   JSON object matching the `MetricSample` schema.
   ```

2. Under `components.schemas:`, add a `MetricSample` schema mirroring the Rust struct exactly:

   ```yaml
   MetricSample:
     type: object
     required:
       - ts_unix_ms
       - cpu_usage_pct
       - ram_used_mib
       - ram_total_mib
       - models_loaded
     properties:
       ts_unix_ms:
         type: integer
         format: int64
         description: Sample timestamp (unix milliseconds).
       cpu_usage_pct:
         type: number
         format: float
       ram_used_mib:
         type: integer
         format: int64
       ram_total_mib:
         type: integer
         format: int64
       gpu_utilization_pct:
         type: integer
         minimum: 0
         maximum: 100
         nullable: true
       vram:
         $ref: "#/components/schemas/VramInfo"
         nullable: true
       models_loaded:
         type: integer
         format: int64
   ```

   If `VramInfo` is not yet defined in `components.schemas`, add it (mirroring the Rust struct in `gpu.rs`: `used_mib: u64`, `total_mib: u64`, both required).

**What NOT to change:**
- Do not modify the existing `SystemHealth` schema or `/koji/v1/system/health` path.

**Steps:**
- [ ] Run `ls docs/openapi/` and inspect `koji-api.yaml` to locate the `paths:` and `components.schemas:` sections.
- [ ] Add the new path entry after `/koji/v1/system/health`.
- [ ] Add the `MetricSample` schema (and `VramInfo` if missing) under `components.schemas`.
- [ ] If a workspace has an OpenAPI linter (search for `redocly`, `spectral`, or `openapi` in `Makefile` and root config files), run it. Otherwise, validate manually with a YAML parser:
  ```bash
  python3 -c "import yaml; yaml.safe_load(open('docs/openapi/koji-api.yaml'))"
  ```
  - Did it parse without error? If not, fix YAML syntax and re-run.
- [ ] Commit with message: `docs(openapi): document /koji/v1/system/metrics/stream and MetricSample`

**Acceptance criteria:**
- [ ] `koji-api.yaml` parses as valid YAML.
- [ ] `/koji/v1/system/metrics/stream` is documented with `text/event-stream` content type.
- [ ] `MetricSample` schema exists under `components.schemas` with all required fields.
- [ ] No existing path or schema was modified.

---

## Task Frontend — replace polling with `EventSource`

**Context:**
Today, `crates/koji-web/src/pages/dashboard.rs` polls `/koji/v1/system/health` every 3 seconds via `web_sys::set_interval` and accumulates a 100-entry in-memory ring buffer. We replace this with a `web_sys::EventSource` connection to `/koji/v1/system/metrics/stream`. Each `sample` event is parsed into a new local `MetricSample` struct (mirror of the backend struct from Task 3) and pushed into the existing `history` `RwSignal`. The 100-entry cap stays — at 2s cadence that's ~3.3 minutes of live data. The status badge in the page header continues to derive from the most recent sample (per user decision, we hard-code `"ok"` as the status string since the existence of recent samples implies health). The "Restart" button is unchanged. The error/retry UX is preserved: if `EventSource.onerror` fires AND the history buffer is empty, we show the existing failure card with a Retry button that re-creates the EventSource.

`web-sys` features must be expanded — currently `koji-web/Cargo.toml` line 13 only enables `Window`, `Document`, `HtmlElement`, `HtmlInputElement`. We need `EventSource`, `MessageEvent`, and `Event` (for the error handler closure type).

**Files:**
- Modify: `crates/koji-web/Cargo.toml` (add `web-sys` features)
- Modify: `crates/koji-web/src/pages/dashboard.rs` (replace polling logic)

**What to implement:**

1. **`web-sys` features** in `crates/koji-web/Cargo.toml` line 13. Change to:
   ```toml
   web-sys = { workspace = true, features = ["Window", "Document", "HtmlElement", "HtmlInputElement", "EventSource", "EventSourceInit", "MessageEvent", "Event"] }
   ```

2. **`MetricSample` and `VramInfo` structs** in `crates/koji-web/src/pages/dashboard.rs`. Replace the existing `SystemHealth` and `VramInfo` structs with:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   struct MetricSample {
       ts_unix_ms: i64,
       cpu_usage_pct: f32,
       ram_used_mib: u64,
       ram_total_mib: u64,
       gpu_utilization_pct: Option<u8>,
       vram: Option<VramInfo>,
       models_loaded: u64,
   }

   #[derive(Debug, Clone, Serialize, Deserialize)]
   struct VramInfo {
       used_mib: u64,
       total_mib: u64,
   }
   ```
   The `status` and `service` fields are dropped — they were only displayed as a hard-coded badge. The badge now displays `"ok"` (hard-coded) whenever there is at least one sample in the history buffer.

3. **Replace the body of `Dashboard()`**. The new structure:

   ```rust
   #[component]
   pub fn Dashboard() -> impl IntoView {
       let history = RwSignal::new(Vec::<MetricSample>::new());
       let fetch_failed = RwSignal::new(false);
       // Incrementing this signal re-runs the Effect that opens the EventSource.
       let connect_trigger = RwSignal::new(0u32);

       // Open (or re-open) an EventSource each time connect_trigger changes.
       Effect::new(move |_| {
           let _ = connect_trigger.get(); // track signal

           let es = match web_sys::EventSource::new("/koji/v1/system/metrics/stream") {
               Ok(es) => es,
               Err(_) => {
                   fetch_failed.set(true);
                   return;
               }
           };

           // Handler for "sample" events.
           let on_sample = Closure::<dyn Fn(web_sys::MessageEvent)>::new(
               move |evt: web_sys::MessageEvent| {
                   if let Some(data_str) = evt.data().as_string() {
                       if let Ok(sample) = serde_json::from_str::<MetricSample>(&data_str) {
                           fetch_failed.set(false);
                           history.update(|buf| {
                               buf.push(sample);
                               if buf.len() > 100 {
                                   buf.drain(..buf.len() - 100);
                               }
                           });
                       }
                   }
               },
           );
           let _ = es.add_event_listener_with_callback(
               "sample",
               on_sample.as_ref().unchecked_ref(),
           );
           on_sample.forget();

           // Error handler — flag for the empty-history retry UI.
           let on_error =
               Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
                   fetch_failed.set(true);
               });
           es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
           on_error.forget();

           // Close the EventSource when the effect re-runs or the component unmounts.
           on_cleanup(move || {
               es.close();
           });
       });

       // Manual retry: close and re-open the EventSource.
       let manual_refresh = move |_| {
           fetch_failed.set(false);
           connect_trigger.update(|n| *n += 1);
       };

       let restart: Action<(), (), LocalStorage> =
           Action::new_unsync(|_: &()| async move {
               let _ = gloo_net::http::Request::post("/koji/v1/system/restart")
                   .send()
                   .await;
           });

       view! { /* same layout as today — see view changes below */ }
   }
   ```

   **View changes from the current dashboard.rs:**
   - **Page header badge**: replace `status_badge_class(&h.status)` / `h.status.clone()` with the hard-coded literal `"badge-success"` / `"ok"`. Remove `status_badge_class` helper entirely (it's no longer referenced).
   - **CPU / Memory / GPU / VRAM sparkline cards**: unchanged structurally. All field names (`cpu_usage_pct`, `ram_used_mib`, `ram_total_mib`, `gpu_utilization_pct`, `vram`) exist on `MetricSample` with the same types.
   - **Models Loaded card**: `h.models_loaded` is now `u64` (was `usize`). `{}` formatting works for both; no change needed.
   - **Error card**: condition `fetch_failed.get() && buf.is_empty()` and the Retry button call `manual_refresh` — unchanged.
   - **Loading spinner**: condition `buf.last().cloned()` returning `None` — unchanged.

4. **Remove**:
   - The `setInterval` + `Closure` block (timer that increments `refresh` every 3s).
   - The `LocalResource::new` block that fetches `/koji/v1/system/health`.
   - The `Effect::new` accumulator that pushed into `history` from the resource.
   - The `manual_refresh` closure that incremented `refresh` (replaced by the new one above).
   - The `status_badge_class` function.
   - Any `use` imports that become unused (`wasm_bindgen::prelude::*` may still be needed for `LocalStorage` and `Closure`; verify).

**What NOT to change:**
- Do not touch `crates/koji-web/src/components/sparkline.rs`.
- Do not touch `crates/koji-web/style.css`.
- Do not touch any other page under `crates/koji-web/src/pages/`.

**Steps:**
- [ ] Update `crates/koji-web/Cargo.toml` line 13 to add the new `web-sys` features.
- [ ] Run `cargo check --package koji-web` to confirm the feature names are valid.
  - Did it succeed? If not, check `web-sys` docs for correct feature names and re-try.
- [ ] Replace the struct definitions (`SystemHealth` → `MetricSample`, update `VramInfo`).
- [ ] Replace the `Dashboard` component body per the structure above.
- [ ] Remove `status_badge_class`, the `setInterval` block, the `LocalResource`, and the old `Effect`.
- [ ] Update the view to hard-code `"ok"` / `"badge-success"` in the header badge.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package koji-web`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package koji-core --package koji-cli --package koji-mock -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Manually verify (if `trunk serve` is available): start the koji proxy, then `cd crates/koji-web && trunk serve`, open the dashboard, confirm graphs populate within ~2 seconds and continue updating.
- [ ] Commit with message: `feat(web): stream dashboard metrics over SSE instead of polling`

**Acceptance criteria:**
- [ ] Dashboard opens an `EventSource` to `/koji/v1/system/metrics/stream` on mount.
- [ ] Each received `sample` event appends to the history buffer (cap 100).
- [ ] Sparkline charts render the same way as today.
- [ ] When the EventSource errors and history is empty, the retry UI shows.
- [ ] Clicking Retry re-creates the EventSource (via `connect_trigger`).
- [ ] No `setInterval` or `LocalResource`-based polling remains in `dashboard.rs`.
- [ ] EventSource is closed on component unmount via `on_cleanup`.
- [ ] The status badge shows `"ok"` whenever there is at least one sample.
- [ ] `status_badge_class` helper is removed.

---

## Task Mark the prior in-memory plan as superseded

**Context:**
`docs/plans/2026-04-06-dashboard-time-series-graphs.md` describes the in-memory ring buffer approach that this plan supersedes. Adding a one-line note prevents future readers from mistaking it for current architecture.

**Files:**
- Modify: `docs/plans/2026-04-06-dashboard-time-series-graphs.md`

**What to implement:**

Add a single blockquote immediately after the `# Dashboard Time-Series Graphs Plan` heading (before "**Goal:**"):

```markdown
> **Status:** Superseded by `docs/plans/2026-04-06-persist-dashboard-metrics.md`. The in-memory ring buffer described below has been replaced by SQLite persistence + an SSE stream.
```

**Steps:**
- [ ] Add the note to the top of the file as described.
- [ ] Commit with message: `docs(plans): mark in-memory dashboard plan as superseded`

**Acceptance criteria:**
- [ ] The superseded note is the first content under the `#` heading.
- [ ] No other content was modified.

---

## File-change summary

| File | Task | Change |
|---|---|---|
| `crates/koji-core/src/config/types.rs` | 1 | Add `metrics_retention_secs` field + default |
| `config/koji.toml*` | 1 | Document the new key |
| `crates/koji-core/src/db/migrations.rs` | 2 | Migration v4, bump `LATEST_VERSION` |
| `crates/koji-core/src/db/queries.rs` | 2 | `SystemMetricsRow` + 3 helpers + tests |
| `crates/koji-core/src/db/mod.rs` | 2 | Migration v4 test |
| `crates/koji-core/src/gpu.rs` | 3 | `MetricSample` struct |
| `crates/koji-core/src/proxy/types.rs` | 3 | Add `metrics_tx` field to `ProxyState` |
| `crates/koji-core/src/proxy/state.rs` | 3 | Initialize `metrics_tx` in `new` |
| `crates/koji-core/src/proxy/server/mod.rs` | 4 | 2s tick, persist + broadcast |
| `crates/koji-core/src/proxy/koji_handlers.rs` | 5 | `handle_system_metrics_stream` handler |
| `crates/koji-core/src/proxy/server/router.rs` | 5 | Wire `/koji/v1/system/metrics/stream` |
| `docs/openapi/koji-api.yaml` | 6 | Document endpoint + `MetricSample` schema |
| `crates/koji-web/Cargo.toml` | 7 | Add `EventSource`/`MessageEvent`/`Event`/`EventSourceInit` web-sys features |
| `crates/koji-web/src/pages/dashboard.rs` | 7 | Replace polling with `EventSource` |
| `docs/plans/2026-04-06-dashboard-time-series-graphs.md` | 8 | Superseded note |

## Risks and tradeoffs

1. **2s polling more than doubles `nvidia-smi` invocations** vs the current 5s. Each call is <10 ms and already runs on a blocking thread. Negligible in practice but noted.
2. **DB growth:** ~80 bytes/row × 30 rows/min × 60 × 24 ≈ **3.5 MB/day**. Inline pruning keeps the row count bounded; SQLite WAL mode may not shrink the file but the page count stays bounded after the first cycle.
3. **Broadcast capacity 64**: a slow client lagging more than ~2 minutes will receive a `lagged` event. The dashboard ignores it (the next live sample arrives 2s later anyway).
4. **`web-sys::EventSource` feature gate**: if `cargo check` fails after the feature change, try dropping `EventSourceInit` — some `web-sys` versions bundle it with `EventSource`.
5. **Test flakiness around timing**: Tasks 4 and 5 sleep ~2.5–4 seconds to wait for the metrics task. Mark them with `#[ignore]` if they prove flaky on CI and add a faster unit test that drives the broadcast channel directly.
6. **DB lock contention**: SQLite WAL mode handles single-writer/multi-reader well, and our writes run on `spawn_blocking`, so they won't block the runtime.
