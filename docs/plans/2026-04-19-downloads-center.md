# Downloads Center Plan

**Goal:** Implement a persistent download queue with active progress tracking, history, and toast notifications for model downloads.

**Architecture:** A new `download_queue` SQLite table stores queued/running/completed items. A background queue processor in `ProxyState` picks one item at a time (sequential). SSE events push download progress to the browser, which renders them as toasts (top-right) and on the Downloads page.

**Tech Stack:** Rust, SQLite (rusqlite), Axum (SSE), Leptos (WASM SPA)

---

### Task 1: Database migration + queue query layer

**Context:**
Before any download logic can be persisted, we need the `download_queue` table and a query module to read/write it. This is purely infrastructure — no business logic yet. The table mirrors the existing `download_log` audit trail but is operational (updated as status changes) rather than append-only.

**Files:**
- Create: `crates/koji-core/src/db/queries/download_queue_queries.rs`
- Modify: `crates/koji-core/src/db/migrations.rs` (add migration 11, bump LATEST_VERSION to 11)
- Modify: `crates/koji-core/src/db/queries/mod.rs` (export new module)
- Modify: `crates/koji-core/src/db/mod.rs` (export new module)

**What to implement:**

#### Migration 11 — `download_queue` table

Add this migration after migration 10 in the migrations array. Bump `LATEST_VERSION` from 10 to 11.

```sql
CREATE TABLE download_queue (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id        TEXT NOT NULL UNIQUE,          -- "pull-<uuid>"
    repo_id       TEXT NOT NULL,                 -- HF repo, e.g. "unsloth/Qwen3.6-35B-A3B-GGUF"
    filename      TEXT NOT NULL,                 -- file being downloaded
    display_name  TEXT,                          -- human-readable model name (nullable)
    status        TEXT NOT NULL DEFAULT 'queued',-- queued | running | verifying | completed | failed | cancelled
    bytes_downloaded INTEGER NOT NULL DEFAULT 0,
    total_bytes     INTEGER,                     -- may be unknown initially (NULL)
    error_message TEXT,                          -- reason for failure/cancellation
    started_at     TEXT,                         -- ISO 8601, set when status → running
    completed_at   TEXT,                         -- ISO 8601, set on terminal states
    queued_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    kind           TEXT NOT NULL DEFAULT 'model' -- 'model' | 'backend'
);

CREATE INDEX idx_dq_status ON download_queue(status);
```

#### Query types — `DownloadQueueItem`

```rust
#[derive(Debug, Clone)]
pub struct DownloadQueueItem {
    pub id: i64,
    pub job_id: String,
    pub repo_id: String,
    pub filename: String,
    pub display_name: Option<String>,
    pub status: String,   // "queued" | "running" | "verifying" | "completed" | "failed" | "cancelled"
    pub bytes_downloaded: i64,
    pub total_bytes: Option<i64>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub queued_at: String,
    pub kind: String,     // "model" | "backend"
}
```

#### Query functions

All return `anyhow::Result<T>`. Each function takes `&Connection` and uses parameterized queries.

1. **`insert_queue_item(conn, job_id, repo_id, filename, display_name, kind) -> Result<i64>`**
   - INSERT INTO download_queue (job_id, repo_id, filename, display_name, status, kind, queued_at) VALUES (?, ?, ?, ?, 'queued', ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
   - Returns the new row id

2. **`get_queued_item(conn) -> Result<Option<DownloadQueueItem>>`**
   - SELECT * FROM download_queue WHERE status = 'queued' ORDER BY queued_at ASC LIMIT 1
   - Returns the oldest queued item (FIFO)

3. **`update_queue_status(conn, job_id, new_status, bytes_downloaded, total_bytes, error_message) -> Result<()>`**
   - `UPDATE download_queue SET status=?, bytes_downloaded=?, total_bytes=?, error_message=?, started_at=COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), completed_at=CASE WHEN ? IN ('completed','failed','cancelled') THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE completed_at END WHERE job_id = ?`
   - **Parameters in order:** new_status, bytes_downloaded, total_bytes, error_message, terminal_status_check (for CASE expression), job_id (for WHERE clause)
   - Only updates started_at if it's NULL (first time going to running)
   - Only sets completed_at when transitioning to a terminal state

3b. **`try_mark_running(conn, job_id) -> Result<bool>`**
   - `UPDATE download_queue SET status='running', started_at=COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) WHERE job_id = ? AND status = 'queued'`
   - Returns `true` if a row was affected (item was queued, now running), `false` if no row matched (item already started by someone else)
   - This is the **atomic CAS guard** that prevents double-starting downloads. The queue processor calls this before spawning the download — if it returns `false`, the item was already picked up and the processor skips it.
   - **Note:** rusqlite's `execute()` returns the number of rows affected. A return value of `0` means the CAS failed.

3c. **`get_item_by_job_id(conn, job_id) -> Result<Option<DownloadQueueItem>>`**
   - `SELECT * FROM download_queue WHERE job_id = ? LIMIT 1`
   - Used to read a queue item's details (including filename, repo_id, total_bytes) for event emission and API responses.

4. **`get_active_items(conn) -> Result<Vec<DownloadQueueItem>>`**
   - SELECT * FROM download_queue WHERE status IN ('queued', 'running', 'verifying') ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'verifying' THEN 1 ELSE 2 END, queued_at ASC
   - Returns active items (should be 0–1 in sequential mode)

5. **`get_history_items(conn, limit, offset) -> Result<Vec<DownloadQueueItem>>`**
   - SELECT * FROM download_queue WHERE status IN ('completed', 'failed', 'cancelled') ORDER BY completed_at DESC LIMIT ? OFFSET ?
   - Returns history items, sorted newest first

5b. **`count_history_items(conn) -> Result<i64>`**
   - SELECT COUNT(*) FROM download_queue WHERE status IN ('completed', 'failed', 'cancelled')
   - Returns total count for pagination in the API response (DownloadsHistoryResponse.total)

6. **`cancel_queue_item(conn, job_id) -> Result<()>`**
   - UPDATE download_queue SET status = 'cancelled', completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE job_id = ? AND status IN ('queued', 'running', 'verifying')
   - Only cancels items that haven't reached a terminal state

8. **`get_running_item(conn) -> Result<Option<DownloadQueueItem>>`**
   - SELECT * FROM download_queue WHERE status IN ('running', 'verifying') LIMIT 1
   - Used on startup to re-attach any running downloads

9. **`mark_stale_running_as_failed(conn) -> Result<()>`**
   - UPDATE download_queue SET status = 'failed', error_message = 'Download was interrupted (process restart)', completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE status IN ('running', 'verifying') AND completed_at IS NULL
   - Called on startup — any running item without a completed_at is stale (the previous process died)

**Tests to write in `download_queue_queries.rs` under `#[cfg(test)]`:**

1. `test_insert_and_get_queued` — insert an item, verify it's returned by `get_queued_item`
2. `test_update_status_sets_timestamps` — insert, update to running (started_at set), update to completed (completed_at set)
3. `test_get_active_items_ordering` — insert items in various statuses, verify running comes first
4. `test_cancel_queue_item` — insert queued item, cancel it, verify status is 'cancelled'
5. `test_cancel_does_not_affect_completed` — insert completed item, try to cancel, verify no change
6. `test_get_history_items` — insert completed/failed items, verify they're returned sorted by completed_at desc
7. `test_count_history_items` — insert completed/failed items, verify count matches
8. `test_mark_stale_running_as_failed` — insert a running item without completed_at, run the function, verify it's marked failed
9. `test_try_mark_running_succeeds` — insert queued item, call try_mark_running, verify returns true and status changed to 'running'
10. `test_try_mark_running_fails_if_already_started` — insert running item, call try_mark_running, verify returns false
11. `test_get_item_by_job_id` — insert item, retrieve by job_id, verify all fields match

**Steps:**
- [ ] Write failing test for `test_insert_and_get_queued` in `download_queue_queries.rs`
- [ ] Run `cargo test --package koji-core test_insert_and_get_queued` — should fail (module doesn't exist yet)
- [ ] Add migration 11 to `migrations.rs`, bump `LATEST_VERSION` to 11
- [ ] Create `download_queue_queries.rs` with `DownloadQueueItem` struct and stub functions
- [ ] Implement `insert_queue_item` — run `cargo test --package koji-core test_insert_and_get_queued` — should pass
- [ ] Implement `update_queue_status` — add `test_update_status_sets_timestamps` — run and pass
- [ ] Implement `get_active_items` — add `test_get_active_items_ordering` — run and pass
- [ ] Implement `cancel_queue_item` — add `test_cancel_queue_item` and `test_cancel_does_not_affect_completed` — run and pass
- [ ] Implement `get_history_items` — add `test_get_history_items` — run and pass
- [ ] Implement `try_mark_running`, `get_item_by_job_id`, `get_running_item`, and `mark_stale_running_as_failed` — add tests — run and pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --package koji-core -- download_queue_queries` — all pass
- [ ] Commit with message: "feat(db): add download_queue table and query layer (migration 11)"

**Acceptance criteria:**
- [ ] Migration 11 creates `download_queue` table with all specified columns and indexes
- [ ] All 10 query functions exist and work correctly
- [ ] All tests pass
- [ ] `cargo clippy --package koji-core -- -D warnings` passes for the new code

---

### Task 2: DownloadQueueService + background processor

**Context:**
Now that we have a persistent queue, we need a service layer that manages the lifecycle: enqueue items, dequeue the next one, update status during download, and run a background processor that picks up queued items one at a time. The processor also handles startup recovery (marking stale running items as failed).

**Files:**
- Create: `crates/koji-core/src/proxy/download_queue.rs` (service + events — placed in proxy/, not models/)
- Modify: `crates/koji-core/src/proxy/state.rs` (add DownloadQueueService to ProxyState, start processor)
- Modify: `crates/koji-core/src/proxy/types.rs` (add SSE event type for downloads)
- Modify: `crates/koji-core/src/proxy/mod.rs` (export download_queue module)

**What to implement:**

#### `DownloadQueueService` struct (placed in `proxy/`, not `models/`)

**Rationale:** The `models/` directory contains domain data types (`ModelCard`, `PullJob`, `SearchResult`). `DownloadQueueService` is a stateful service with a broadcast channel — it's infrastructure, not a model. Placing it in `proxy/` alongside other proxy infrastructure (like `pull_jobs.rs`) is the correct location.

```rust
pub struct DownloadQueueService {
    db_dir: Option<PathBuf>,
    events_tx: tokio::sync::broadcast::Sender<DownloadEvent>,
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Started { job_id: String, repo_id: String, filename: String, total_bytes: Option<u64> },
    Progress { job_id: String, bytes_downloaded: u64, total_bytes: Option<u64> },  // None if total unknown
    Verifying { job_id: String, filename: String },
    Completed { job_id: String, filename: String, size_bytes: u64, duration_ms: u64 },
    Failed { job_id: String, filename: String, error: String },
    Cancelled { job_id: String, filename: String },
    Queued { job_id: String, repo_id: String, filename: String },
}
```

#### Methods (all synchronous — no `.await`)

**All DB operations are synchronous.** The service opens a `rusqlite::Connection` internally for each method call. `tokio::sync::broadcast::Sender::send()` is also non-async. Do NOT use `.await` on any of these methods.

1. **`new(db_dir: Option<PathBuf>) -> Self`**
   - Creates the service with a broadcast channel (capacity 64)

2. **`enqueue(&self, job_id: &str, repo_id: &str, filename: &str, display_name: Option<&str>, kind: &str) -> Result<()>`**
   - Opens DB connection, calls `insert_queue_item`, emits `DownloadEvent::Queued`
   - Returns `Err` if the job_id already exists (UNIQUE constraint violation)

3. **`dequeue(&self) -> Result<Option<DownloadQueueItem>>`**
   - Opens DB connection, calls `get_queued_item`
   - Returns the oldest queued item (FIFO), or None if queue is empty

4. **`update_status(&self, job_id: &str, new_status: &str, bytes_downloaded: i64, total_bytes: Option<i64>, error_message: Option<&str>, duration_ms: Option<u64>) -> Result<()>`**
   - Opens DB connection, calls `get_item_by_job_id` to read the current row (needed for filename in event emission)
   - Calls `update_queue_status` with the parameters
   - Emits the appropriate `DownloadEvent` based on `new_status`:
     - "running" → `Started { job_id, repo_id, filename, total_bytes }` (filename and repo_id from DB row)
     - "verifying" → `Verifying { job_id, filename }` (filename from DB row)
     - "completed" → `Completed { job_id, filename, size_bytes, duration_ms }` (filename from DB row, size_bytes = bytes_downloaded)
     - "failed" → `Failed { job_id, filename, error }` (filename from DB row)
     - "cancelled" → `Cancelled { job_id, filename }` (filename from DB row)

5. **`cancel(&self, job_id: &str) -> Result<()>`**
   - Opens DB connection, calls `cancel_queue_item`
   - Emits `DownloadEvent::Cancelled`
   - Returns error if item not found or already in terminal state

5. **`get_active_items(&self) -> Result<Vec<DownloadQueueItem>>`**
   - Opens DB connection, calls `get_active_items`
   - Returns active items (queued + running + verifying), ordered by status priority

6. **`subscribe_events(&self) -> tokio::sync::broadcast::Receiver<DownloadEvent>`**
   - Returns a new receiver on the events channel
   - Call this from SSE handler and toast store

7. **`on_startup_recovery(&self) -> Result<()>`**
   - Opens DB connection, calls `mark_stale_running_as_failed`
   - For each stale item that was marked failed, emit `DownloadEvent::Failed`
   - Returns the list of stale job_ids so the caller can clean up in-memory state

#### Queue processor background task

**This is the ONLY code path that transitions items from `queued` → `running`.** `spawn_download_job` does NOT start downloads directly.

```rust
async fn queue_processor_loop(state: Arc<ProxyState>) {
    let svc = match &state.download_queue {
        Some(s) => s,
        None => return, // No DB configured, nothing to do
    };

    // Startup recovery: mark stale running items as failed
    if let Err(e) = svc.on_startup_recovery() {
        tracing::error!(error=%e, "Startup recovery failed");
    }

    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Check if anything is currently running (only one at a time in sequential mode)
        let active = match svc.get_active_items() {
            Ok(items) => items,
            Err(e) => { tracing::error!(error=%e, "Failed to check active downloads"); continue; }
        };

        let has_running = active.iter().any(|item| item.status == "running" || item.status == "verifying");

        if has_running {
            continue; // Something is running, wait for it to finish
        }

        // Try to dequeue the next item
        match svc.dequeue() {
            Some(item) => {
                // Atomic CAS: only transition if still 'queued'. This is the safety guard
                // that prevents double-starts. If another consumer already marked it running,
                // this returns false and we skip.
                let was_queued = match svc.try_mark_running(&item.job_id) {
                    Ok(true) => true,
                    Ok(false) => {
                        tracing::info!(job_id=%item.job_id, "Item already started by another consumer, skipping");
                        continue;
                    }
                    Err(e) => {
                        tracing::error!(error=%e, job_id=%item.job_id, "CAS failed to mark item as running");
                        continue;
                    }
                };

                if was_queued {
                    // Emit Started event (reads filename from DB via get_item_by_job_id)
                    let _ = svc.update_status(&item.job_id, "running", 0, None, None, None);
                    // Spawn the actual download (delegated to a separate async function)
                    let job_id = item.job_id.clone();
                    let state_clone = Arc::clone(&state);
                    tokio::spawn(async move {
                        start_download_from_queue(state_clone, job_id).await;
                    });
                }
            }
            None => { /* queue empty, continue looping */ }
        }
    }
}
```

**Key: `start_download_from_queue` is a NEW function** (not the existing `spawn_download_job`). It:
1. Takes a `job_id` and `Arc<ProxyState>`
2. Opens the DB to read the queue item's details (repo_id, filename, etc.)
3. Performs the actual download work (refactored from `spawn_download_job`)
4. On completion/failure, calls `svc.update_status()` with the final status
5. This function is also callable directly by `spawn_download_job` after enqueueing, as a replacement for direct spawning.

#### Integration into `ProxyState`

Add to `ProxyState`:
```rust
pub download_queue: Option<Arc<DownloadQueueService>>,
```

In `ProxyState::new`, initialize it if `db_dir` is Some:
```rust
let download_queue = db_dir.as_ref().map(|dir| {
    Arc::new(DownloadQueueService::new(Some(dir.clone())))
});
```

Then spawn the queue processor task:
```rust
if let Some(ref dq) = download_queue {
    let processor_state = Arc::clone(self);
    tokio::spawn(async move {
        queue_processor_loop(processor_state).await;
    });
}
```

**Tests:**
1. `test_enqueue_and_dequeue` — enqueue an item, dequeue it, verify fields match
2. `test_update_status_emits_event` — enqueue, update status, verify event was received on channel
3. `test_cancel_emits_event` — enqueue, cancel, verify Cancelled event
4. `test_dequeue_empty_queue_returns_none`

**Steps:**
- [ ] Create `download_queue.rs` with `DownloadQueueService`, `DownloadEvent`, and stub methods
- [ ] Write `test_enqueue_and_dequeue` — run and fail
- [ ] Implement `enqueue`, `dequeue` — run and pass
- [ ] Implement `update_status` with event emission — add `test_update_status_emits_event` — run and pass
- [ ] Implement `cancel` — add `test_cancel_emits_event` — run and pass
- [ ] Add `DownloadEvent` to `proxy/types.rs` (or keep in download_queue.rs, re-export)
- [ ] Add `download_queue` field to `ProxyState` in `types.rs`
- [ ] Initialize it in `state.rs::new()`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --package koji-core -- download_queue` — all pass
- [ ] Commit with message: "feat(core): add DownloadQueueService with event bus and background processor"

**Acceptance criteria:**
- [ ] `DownloadQueueService` manages enqueue/dequeue/cancel with DB persistence
- [ ] `DownloadEvent` enum covers all lifecycle transitions
- [ ] Broadcast channel delivers events to subscribers
- [ ] Service initializes in `ProxyState::new()` when db_dir is configured

---

### Task 3: Hook downloads into the queue (pull handler integration)

**Context:**
Currently `spawn_download_job` in `koji_handlers/pull.rs` creates a `PullJob` in memory and spawns a download directly. We need to refactor this into a two-step process with a **single canonical path** — no race conditions.

**CRITICAL DESIGN DECISION — Single canonical path (no direct start):**
- `enqueue_download()` inserts a `queued` row into the DB and returns immediately.
- The queue processor is the **sole entity** that transitions items from `queued` → `running` by calling `start_download_from_queue()`.
- `enqueue_download()` does NOT call `start_download_from_queue()` directly. If it did, the queue processor would also pick up the item 2 seconds later, causing a double-start race.
- The queue processor uses an atomic CAS: `UPDATE download_queue SET status='running' WHERE job_id=? AND status='queued'`. If this affects 0 rows, the item was already started by someone else (skip it). This prevents double-starts even if multiple consumers exist.

**Files:**
- Modify: `crates/koji-core/src/proxy/pull_jobs.rs` (add `duration_ms` field)
- Modify: `crates/koji-core/src/proxy/koji_handlers/pull.rs` (refactor spawn_download_job, add enqueue_download + start_download_from_queue)
- Modify: `crates/koji-core/src/proxy/state.rs` (queue processor task already started in Task 2)

**What to implement:**

#### PullJob additions

Add to `PullJob`:
```rust
pub duration_ms: Option<u64>,  // Set on completion, calculated via Instant::now().elapsed()
```

#### Refactor `spawn_download_job` into two functions

**Current state:** `spawn_download_job` enqueues a `PullJob` in memory AND spawns the download directly.

**New state — Two separate functions with single canonical path:**

1. **`enqueue_download(job_id, repo_id, filename, display_name, kind) -> Result<()>`** (called from `handle_koji_pull_model`)
   - Creates the `download_queue` DB row via `state.download_queue.enqueue()` with status='queued'
   - Returns immediately — does NOT start the download
   - The queue processor will pick it up and start it

2. **`start_download_from_queue(job_id, state) -> impl Future<Output=()>`** (called ONLY by queue processor)
   - This is the **refactored body of `spawn_download_job`**, moved into a standalone async function
   - Takes a `job_id` and `Arc<ProxyState>`
   - Opens the DB to read the queue item's details (repo_id, filename, etc.)
   - Performs the actual download work (current `spawn_download_job` logic)
   - On completion/failure, calls `state.download_queue.update_status()` with the final status
   - This is the ONLY path that starts a download

**Hook in `handle_koji_pull_model`:**
1. Generate the job_id (existing logic)
2. Look up display_name from model config if available
3. Call `enqueue_download(job_id, repo_id, filename, display_name, "model")`
4. Return the job_id to the client immediately
5. The queue processor picks it up and calls `start_download_from_queue`

**Hook in `spawn_download_job` (the old function):**
- **Remove entirely.** Replace all callers with `enqueue_download` + let the queue processor handle starting.

#### Duration calculation detail

The `Completed` event carries `duration_ms` computed via `Instant::elapsed()`. The DB stores `started_at` and `completed_at` as ISO 8601 strings for persistence. Do NOT try to subtract ISO 8601 strings in SQLite — compute duration in Rust before storing in the event.

#### update_status method signature (includes duration_ms)

The `update_status` method on `DownloadQueueService` must accept `duration_ms` as a parameter so it can emit the `Completed` event with the correct value:
```rust
fn update_status(&self, job_id: &str, new_status: &str, bytes_downloaded: i64, total_bytes: Option<i64>, error_message: Option<&str>, duration_ms: Option<u64>) -> Result<()>
```
When `new_status == "completed"`, the service emits `DownloadEvent::Completed { job_id, filename, size_bytes, duration_ms }`.

**Tests:**
1. `test_enqueue_download_creates_queue_row` — verify that calling enqueue_download creates a download_queue row
2. `test_start_download_from_queue_updates_status` — simulate a full download lifecycle and verify DB state transitions
3. `test_duration_ms_computed_via_instant` — verify duration is computed via `Instant::elapsed()` not string subtraction

Since these are integration tests that require actual file I/O, write them against the in-memory DB.

**Steps:**
- [ ] Add `duration_ms: Option<u64>` to `PullJob` struct
- [ ] Refactor `spawn_download_job` into `enqueue_download` + `start_download_from_queue`
- [ ] Update `handle_koji_pull_model` to call `enqueue_download` instead of spawning directly
- [ ] Update `start_download_from_queue` to call `svc.update_status()` on completion/failure
- [ ] Start queue processor task in `ProxyState::new()` (already covered in Task 2)
- [ ] Write integration test for full download lifecycle through the queue
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --package koji-core` — all pass
- [ ] Commit with message: "feat(core): integrate pull handler with download queue"

**Acceptance criteria:**
- [ ] Every pull creates a `download_queue` row
- [ ] Queue status transitions match download lifecycle (queued → running → verifying → completed/failed)
- [ ] Duration is calculated and stored on completion
- [ ] Queue processor picks up items sequentially
- [ ] Stale running items are marked failed on startup

---

### Task 4: Web API endpoints for downloads

**Context:**
The web UI needs REST endpoints to query the download queue (active + history) and cancel items. We also need an SSE endpoint so the browser receives real-time toast events.

**Files:**
- Create: `crates/koji-web/src/api/downloads.rs`
- Modify: `crates/koji-web/src/api.rs` (add `pub mod downloads;` — NEW, reviewer pointed out this was missing)
- Modify: `crates/koji-web/src/server.rs` (register routes, add AppState field)

**What to implement:**

#### DTO types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadQueueItemDto {
    pub job_id: String,
    pub repo_id: String,
    pub filename: String,
    pub display_name: Option<String>,
    pub status: String,
    pub bytes_downloaded: i64,
    pub total_bytes: Option<i64>,
    pub error_message: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub queued_at: String,
    pub kind: String,
    pub progress_percent: f64,  // computed: bytes_downloaded / total_bytes * 100.0 (0.0 if unknown)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadsActiveResponse {
    pub items: Vec<DownloadQueueItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadsHistoryResponse {
    pub items: Vec<DownloadQueueItemDto>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadCancelResponse {
    pub ok: bool,
    pub message: Option<String>,
}
```

#### API handlers

1. **`GET /api/downloads/active` → `DownloadsActiveResponse`**
   - Calls `download_queue.get_active_items()`
   - Converts each row to `DownloadQueueItemDto` with computed `progress_percent`

2. **`GET /api/downloads/history?limit=50&offset=0` → `DownloadsHistoryResponse`**
   - Parses optional query params (default limit=50, offset=0)
   - Calls `download_queue.get_history_items(limit, offset)`
   - Also returns total count for pagination

3. **`POST /api/downloads/:job_id/cancel` → `DownloadCancelResponse`**
   - Calls `download_queue.cancel(job_id)`
   - Returns ok=true on success, or error message if item not found/already terminal

4. **`GET /api/downloads/events` → SSE stream**
   - Subscribes to `download_queue.subscribe_events()`
   - Streams `DownloadEvent` as SSE events with event names matching the variant:
     - `Started`, `Progress`, `Verifying`, `Completed`, `Failed`, `Cancelled`, `Queued`
   - Each event data is a JSON object with fields appropriate to the event type
   - KeepAlive: 30 seconds

#### SSE Event JSON format

Each event uses standard SSE format with a **typed** `event:` field. The server sends:
```
event: Started
data: {"job_id":"pull-abc","repo_id":"unsloth/Qwen","filename":"Q4.gguf","total_bytes":3200000000}


event: Progress
data: {"job_id":"pull-abc","bytes_downloaded":2100000000,"total_bytes":3200000000}


event: Completed
data: {"job_id":"pull-abc","filename":"Q4.gguf","size_bytes":3200000000,"duration_ms":83000}


event: Failed
data: {"job_id":"pull-abc","filename":"Q4.gguf","error":"LFS hash mismatch"}
```

**Client-side:** Use `web_sys::EventSource::add_event_listener` for each event type. The `onmessage` handler will NOT receive typed events — only untyped ones. Register a listener per event type:
```rust
// For each event type, register a typed listener
for event_name in ["Started", "Progress", "Verifying", "Completed", "Failed", "Cancelled", "Queued"] {
    let handler = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
        if let Some(data) = event.data() {
            let event_json: serde_json::Value = serde_json::from_str(&data).expect("Invalid JSON");
            // Dispatch to toast store and update active_downloads signal
            match event_name {
                "Started" | "Progress" | "Verifying" => { /* update active list */ }
                "Queued" => { /* add to active_downloads with status 'queued' */ }
                "Completed" => { toast_store.add(completed_toast); /* update active list */ }
                "Failed" => { toast_store.add(error_toast); /* update active list */ }
                "Cancelled" => { toast_store.add(cancelled_toast); /* update active list */ }
                _ => {}
            }
        }
    }) as Box<dyn FnMut(_)>);
    es.add_event_listener_with_callback(event_name, handler.as_ref().unchecked_ref()).unwrap();
    handler.forget();
}
```

#### AppState addition

Add to `AppState`:
```rust
pub download_queue: Option<Arc<koji_core::proxy::download_queue::DownloadQueueService>>,
```

Initialize it in `run_with_opts` from the proxy config's state (extract `db_dir` from proxy state).

**Tests:**
1. Test `GET /api/downloads/active` returns correct DTOs
2. Test `GET /api/downloads/history` with pagination
3. Test `POST /api/downloads/:id/cancel` succeeds for queued item
4. Test `POST /api/downloads/:id/cancel` returns error for already completed item

These are integration tests using the existing test infrastructure (similar to `tests/backends_api.rs`).

**Steps:**
- [ ] Create `downloads.rs` with DTO types and stub handlers
- [ ] Implement `GET /api/downloads/active` handler
- [ ] Implement `GET /api/downloads/history` handler with query params
- [ ] Implement `POST /api/downloads/:job_id/cancel` handler
- [ ] Implement `GET /api/downloads/events` SSE handler
- [ ] Add `download_queue` field to `AppState` in `server.rs`
- [ ] Register routes in `build_router`
- [ ] Write integration tests
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --package koji-web` — all pass
- [ ] Commit with message: "feat(web): add downloads API endpoints (active, history, cancel, SSE)"

**Acceptance criteria:**
- [ ] 4 API endpoints registered and functional
- [ ] SSE endpoint streams download events to browser subscribers
- [ ] DTOs include computed `progress_percent`
- [ ] History endpoint supports pagination with limit/offset query params
- [ ] Cancel endpoint only affects non-terminal items

---

### Task 5: Frontend — Toast component + Downloads page

**Context:**
Now we build the UI. Two components: a toast notification system (top-right, auto-dismissing) and a Downloads page (Active/History tabs). The browser opens an SSE connection on app mount to receive download events, which trigger toasts.

**Files:**
- Create: `crates/koji-web/src/components/toast.rs`
- Create: `crates/koji-web/src/pages/downloads.rs`
- Modify: `crates/koji-web/src/components/mod.rs` (export toast)
- Modify: `crates/koji-web/src/pages/mod.rs` (export downloads)
- Modify: `crates/koji-web/src/lib.rs` (add /downloads route, wire up SSE + toast store in App)
- Modify: `crates/koji-web/src/components/sidebar.rs` (add Downloads nav item with badge)

**What to implement:**

#### Toast component (`components/toast.rs`)

```rust
// A global toast store accessible from any component
#[derive(Debug, Clone)]
pub struct ToastStore {
    toasts: RwSignal<Vec<Toast>>,
}

#[derive(Debug, Clone)]
pub struct Toast {
    id: String,
    severity: ToastSeverity,  // Info | Success | Warning | Error
    title: String,            // e.g., "Q4_K_M.gguf"
    message: String,          // e.g., "67% — 2.1 / 3.2 GB"
    duration_secs: u64,
    action_label: Option<String>,   // e.g., "Retry", "View"
    on_action: Option<Callback<()>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToastSeverity {
    Info,
    Success,
    Warning,
    Error,
}
```

**Toast store methods:**
- `add(toast: Toast)` — adds a toast, starts auto-dismiss timer via `wasm_bindgen_futures::spawn_local`
- `remove(id: &str)` — removes a toast by id
- `clear()` — removes all toasts
- `from_download_event(event: &DownloadEvent) -> Option<Toast>` — converts an SSE event to a toast (some events don't produce toasts, like Progress)

**Toast rendering:**
- Fixed position div at top-right (`position: fixed; top: 16px; right: 16px; z-index: 9999`)
- Each toast is a styled card with severity-based color border/background
- Auto-dismiss after duration_secs (use `wasm_bindgen_futures::spawn_local` + `gloo_timers::future::TimeoutFuture`)
- Hovering pauses the auto-dismiss timer
- Action button calls the callback if present
- Maximum 5 toasts visible at once (oldest removed when exceeded)

**Event-to-toast mapping:**
| Event | Toast? | Severity | Title | Message | Action |
|-------|--------|----------|-------|---------|--------|
| Queued | No | — | — | — | — |
| Started | Yes | Info | filename | "Downloading..." | — |
| Progress | No | — | — | — | — |
| Verifying | No (or subtle) | Info | filename | "Verifying hash..." | — |
| Completed | Yes | Success | filename | "{size} in {duration}" | View (navigates to /downloads) |
| Failed | Yes | Error | filename | "{error}" | Retry (re-enqueues) |
| Cancelled | Yes | Warning | filename | "Download cancelled" | — |

#### Downloads page (`pages/downloads.rs`)

```rust
#[component]
pub fn Downloads() -> impl IntoView {
    // Signal for active tab selection
    let active_tab = RwSignal::new("active"); // "active" | "history"
    
    // Active downloads
    let active_downloads = RwSignal::new(Vec::<DownloadQueueItemDto>::new());
    let active_loading = RwSignal::new(false);
    
    // History
    let history_items = RwSignal::new(Vec::<DownloadQueueItemDto>::new());
    let history_total = RwSignal::new(0i64);
    let history_page = RwSignal::new(0);
    let history_limit = RwSignal::new(50);
    
    // IMPORTANT: Do NOT use `gloo_net::http::Request::get` for SSE — that performs a normal HTTP request and will buffer/complete rather than streaming.
    // Use `web_sys::EventSource` for proper SSE streaming:
    let es = web_sys::EventSource::new("/api/downloads/events").expect("Failed to create EventSource");
    
    let toast_store = ToastStore::global();
    
    // Register typed event listeners (onmessage won't receive typed events)
    for event_name in ["Started", "Progress", "Verifying", "Completed", "Failed", "Cancelled", "Queued"] {
        let handler = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
            if let Some(data) = event.data() {
                let event_json: serde_json::Value = serde_json::from_str(&data).expect("Invalid JSON");
                match event_name {
                    "Started" | "Progress" | "Verifying" => {
                        // Update active_downloads signal with current state
                    }
                    "Queued" => {
                        // Add to active_downloads with status 'queued'
                    }
                    "Completed" => {
                        toast_store.add(Toast { severity: ToastSeverity::Success, title: ..., message: ... });
                        // Remove from active_downloads
                    }
                    "Failed" => {
                        toast_store.add(Toast { severity: ToastSeverity::Error, title: ..., message: ... });
                        // Remove from active_downloads
                    }
                    "Cancelled" => {
                        toast_store.add(Toast { severity: ToastSeverity::Warning, title: ..., message: ... });
                        // Remove from active_downloads
                    }
                    _ => {}
                }
            }
        }) as Box<dyn FnMut(_)>);
        es.add_event_listener_with_callback(event_name, handler.as_ref().unchecked_ref()).unwrap();
        handler.forget();
    }
    
    // Initial fetch of active downloads (SSE may take a moment to connect)
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(resp) = gloo_net::http::Request::get("/api/downloads/active").send().await {
            if let Ok(data) = resp.json::<DownloadsActiveResponse>().await {
                active_downloads.set(data.items);
            }
        }
    });
    
    view! {
        <div class="page downloads-page">
            <h1 class="page__title">"Downloads Center"</h1>
            
            // Tab navigation
            <div class="downloads-tabs">
                <button class=move || format!("tab-btn {}", if active_tab.get() == "active" { "active" } else { "" })
                    on:click=move |_| active_tab.set("active")>
                    "Active"
                </button>
                <button class=move || format!("tab-btn {}", if active_tab.get() == "history" { "active" } else { "" })
                    on:click=move |_| active_tab.set("history")>
                    "History"
                </button>
            </div>
            
            // Active tab content
            {move || if active_tab.get() == "active" {
                view! {
                    <div class="downloads-active">
                        {move || {
                            let items = active_downloads.get();
                            if items.is_empty() {
                                view! { <p class="empty-state">"No active downloads"</p> }.into_any()
                            } else {
                                items.into_iter().map(|item| render_download_item(item)).collect::<Vec<_>>()
                            }
                        }}
                    </div>
                }.into_any()
            } else {
                view! { <div class="downloads-history">/* history content */</div> }.into_any()
            }}
        </div>
    }
}
```

**Active item rendering:**
- Progress bar with percentage
- Status badge (Downloading, Verifying)
- Bytes / total bytes
- Elapsed time and estimated remaining
- Cancel button (calls `POST /api/downloads/:job_id/cancel`)

**History tab rendering:**
- Sort controls (newest first by default)
- Filter controls (All / Completed / Failed / Cancelled)
- Search box (filters by filename or repo_id)
- Paginated list of history items
- Each item shows: status icon, display name, filename, size, duration, timestamp

#### Sidebar integration

Add a Downloads nav item after Updates:
```html
<A href="/downloads" ...>
    <span class="sidebar-item__icon">"📥"</span>
    <span class="sidebar-item__text">"Downloads"</span>
    {move || queued_count.get().then(|| view! { <span class="sidebar-badge">{queued_count.get()}</span> })}
</A>

The `queued_count` is derived from the active downloads list: count items with status "queued". Update the sidebar to poll `/api/downloads/active` on mount and every 10 seconds (not continuously — only for the badge). The Downloads page itself uses SSE, not polling.
```

The `queued_count` is derived from polling `/api/downloads/active` and counting items with status "queued". Update the sidebar to poll this on mount and every 10 seconds.

**Steps:**
- [ ] Create `toast.rs` with `ToastStore`, `Toast`, `ToastSeverity` types and rendering
- [ ] Implement `from_download_event()` conversion logic
- [ ] Implement auto-dismiss with hover-pause
- [ ] Create `downloads.rs` with `Downloads` component, tab navigation, active list
- [ ] Implement history tab with pagination, sort, filter, search
- [ ] Wire up SSE connection in `lib.rs` App component to dispatch events to toast store
- [ ] Add `/downloads` route in `lib.rs`
- [ ] Update sidebar with Downloads nav item and queued count badge
- [ ] Add CSS classes for toast and downloads page (inline or in existing styles)
- [ ] Test manually: start koji, trigger a download, verify toasts appear and Downloads page updates
- [ ] Commit with message: "feat(web): add Downloads page and toast notifications"

**Acceptance criteria:**
- [ ] Toast component renders top-right, auto-dismisses, supports actions
- [ ] SSE events trigger appropriate toasts (Started → Info, Completed → Success, Failed → Error)
- [ ] Downloads page shows Active tab with progress bar for in-progress download
- [ ] Downloads page shows History tab with pagination and filters
- [ ] Sidebar shows Downloads nav item with queued count badge
- [ ] Cancel button works from Downloads page

---

## Summary of Files Changed

| File | Action |
|------|--------|
| `crates/koji-core/src/db/migrations.rs` | Modify — add migration 11 |
| `crates/koji-core/src/db/queries/mod.rs` | Modify — export new module |
| `crates/koji-core/src/db/mod.rs` | Modify — export new module |
| `crates/koji-core/src/db/queries/download_queue_queries.rs` | **New** — CRUD queries |
| `crates/koji-core/src/proxy/download_queue.rs` | **New** — service + events (placed in proxy/, not models/) |
| `crates/koji-core/src/proxy/pull_jobs.rs` | Modify — add duration_ms field |
| `crates/koji-core/src/proxy/koji_handlers/pull.rs` | Modify — hook into queue |
| `crates/koji-core/src/proxy/state.rs` | Modify — add DownloadQueueService, start processor |
| `crates/koji-core/src/proxy/types.rs` | Modify — add DownloadEvent type |
| `crates/koji-core/src/proxy/mod.rs` | Modify — export download_queue module |
| `crates/koji-web/src/api/downloads.rs` | **New** — API handlers |
| `crates/koji-web/src/api.rs` | Modify — add `pub mod downloads;` |
| `crates/koji-web/src/server.rs` | Modify — register routes, add AppState field |
| `crates/koji-web/src/components/toast.rs` | **New** — toast notifications |
| `crates/koji-web/src/components/mod.rs` | Modify — export toast |
| `crates/koji-web/src/pages/downloads.rs` | **New** — Downloads page |
| `crates/koji-web/src/pages/mod.rs` | Modify — export downloads |
| `crates/koji-web/src/lib.rs` | Modify — add route, wire SSE + toast store |
| `crates/koji-web/src/components/sidebar.rs` | Modify — add Downloads nav item |
