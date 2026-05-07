# Shared Activity Panel + SSE Core Plan

**Goal:** Extract duplicated SSE reconnection logic into a shared utility and create a generic `ActivityPanel` UI shell component, eliminating ~200 lines of duplicated code across `JobLogPanel` and `pull_quant_wizard`.

**Architecture:** Three-layer separation: (1) `sse_stream` utility handles EventSource lifecycle + exponential backoff reconnection, (2) `ActivityPanel` is a presentational UI shell (header + scrollable body), (3) consumers (`JobLogPanel`, `pull_quant_wizard`) compose both layers with their own content and callbacks.

**Tech Stack:** Rust, Leptos (RwSignal, view! macro), `gloo_net::eventsource::futures::EventSource`, `futures_util::future::select`, `wasm_bindgen_futures::spawn_local`

---

## Pre-work: Module Structure

Rename `utils.rs` to `utils/mod.rs`. Since `utils/` already exists (for `self_update.rs`) and `utils.rs` already declares `pub mod self_update;`, this is a pure filename change. All existing imports (`crate::utils::format_size`, `crate::utils::post_request`, etc.) continue to resolve identically.

---

### Task 1: Convert `utils.rs` to `utils/mod.rs` and create `sse_stream.rs`

**Context:**
The `src/utils.rs` file contains helper functions (CSRF, formatting, etc.) and already declares `pub mod self_update;` for the `utils/self_update.rs` submodule. To add `sse_stream.rs`, we need to convert `utils.rs` to `utils/mod.rs`. This is a pure rename with no logic changes - all existing imports (`crate::utils::format_size`, `crate::utils::post_request`, etc.) continue to work because Rust treats `utils/mod.rs` identically to `utils.rs`.

**Files:**
- Create: `crates/tama-web/src/utils/sse_stream.rs`
- Rename: `crates/tama-web/src/utils.rs` → `crates/tama-web/src/utils/mod.rs`

**What to implement:**

1. **Rename:** Move `crates/tama-web/src/utils.rs` to `crates/tama-web/src/utils/mod.rs`. Do NOT change any content - this is a pure file rename.

2. **Create `crates/tama-web/src/utils/sse_stream.rs`** with the following public API:

```rust
use futures_util::Stream;
use leptos::prelude::RwSignal;

/// Configuration for SSE reconnection behavior.
#[derive(Debug, Clone)]
pub struct SseReconnectConfig {
    pub initial_delay_ms: u32,
    pub max_delay_ms: u32,
    pub max_attempts: Option<u32>,
}

impl Default for SseReconnectConfig {
    fn default() -> Self {
        Self {
            initial_delay_ms: 1_000,
            max_delay_ms: 30_000,
            max_attempts: None, // infinite
        }
    }
}

/// Error type for SSE operations.
#[derive(Debug, Clone)]
pub enum SseError {
    /// Failed to create the EventSource connection.
    ConnectionFailed(String),
    /// Failed to subscribe to a channel.
    SubscribeFailed(String),
    /// The connection was closed by the caller.
    Closed,
}

impl std::fmt::Display for SseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SseError::ConnectionFailed(msg) => write!(f, "SSE connection failed: {}", msg),
            SseError::SubscribeFailed(msg) => write!(f, "SSE subscribe failed: {}", msg),
            SseError::Closed => write!(f, "SSE connection closed"),
        }
    }
}

/// A single SSE event received from a channel.
pub struct SseEvent {
    pub event_type: String,  // e.g. "log", "status", "result"
    pub data: String,        // extracted from MessageEvent.data().as_string()
}

/// A stream of SSE events for a single channel.
/// Yields Result<SseEvent, SseError> - Err signals disconnection.
pub struct SseStream {
    // wraps the gloo_net EventSourceSubscription stream internally
}

impl Stream for SseStream {
    type Item = Result<SseEvent, SseError>;
    // delegates to the underlying gloo_net stream,
    // mapping Result<(String, MessageEvent), EventSourceError> to Result<SseEvent, SseError>
}

/// Handle to an SSE connection with automatic reconnection.
/// Implements Drop to auto-cancel the reconnection loop.
///
/// NOT Clone - one connection per URL. Consumer owns the reconnection loop
/// and calls `connect_once()` + `subscribe()` inside its own loop.
pub struct SseConnection {
    url: String,
    config: SseReconnectConfig,
    cancelled: RwSignal<bool>,
    is_reconnecting: RwSignal<bool>,
    last_error: RwSignal<Option<String>>,
    abort_handle: futures_util::future::AbortHandle,
}

impl SseConnection {
    /// Attempt to open (or re-open) the EventSource connection.
    /// Returns Ok(()) when the EventSource is established.
    /// Returns Err on permanent failure (max_attempts exhausted).
    ///
    /// The caller is responsible for the reconnection loop:
    /// ```
    /// loop {
    ///     conn.connect_once().await?;  // waits for connection
    ///     let stream = conn.subscribe("log")?;
    ///     process_events(stream).await;
    ///     // stream ended → loop back for reconnection
    /// }
    /// ```
    pub async fn connect_once(&mut self) -> Result<(), SseError>;

    /// Subscribe to a single event channel.
    /// Must be called AFTER a successful `connect_once()`.
    /// Returns a Stream that yields events until the channel ends
    /// (e.g., EventSource disconnects).
    /// The caller is responsible for multiplexing multiple streams
    /// using futures_util::future::select.
    pub fn subscribe(&self, channel: &str) -> Result<SseStream, SseError>;

    /// Reactive: true while the connection is in backoff/wait state.
    pub fn is_reconnecting(&self) -> RwSignal<bool>;

    /// Reactive: last error message from connection attempts.
    pub fn last_error(&self) -> RwSignal<Option<String>>;

    /// Immediately cancel the reconnection loop.
    /// Uses futures_util::future::AbortHandle to wake from in-flight TimeoutFuture.
    pub fn close(&self);
}

impl Drop for SseConnection {
    fn drop(&mut self) {
        self.close();
    }
}

/// Create a new SSE connection handle.
/// Does NOT open the connection - call `conn.connect_once().await` to connect.
///
/// The `cancelled` signal is checked each loop iteration. When set to `true`,
/// `connect_once()` returns Err(SseError::Closed) immediately.
pub fn create(
    url: String,
    cancelled: RwSignal<bool>,
    config: Option<SseReconnectConfig>,
) -> SseConnection;
```

**Design: Consumer-driven reconnection (Option B)**

The `SseConnection` does NOT own a reconnection loop. Instead it provides `connect_once()` - an async method that attempts to open the EventSource with exponential backoff on failure. The consumer owns the reconnection loop, calling `connect_once()` → `subscribe()` → process events → loop back on stream end.

This is the simplest design: it preserves the current consumer loop structure while extracting the backoff timing and reactive state into `SseConnection`.

**`SseConnection` struct fields:**
```rust
pub struct SseConnection {
    url: String,
    config: SseReconnectConfig,
    cancelled: RwSignal<bool>,
    is_reconnecting: RwSignal<bool>,
    last_error: RwSignal<Option<String>>,
    abort_handle: std::cell::Cell<futures_util::future::AbortHandle>,
    abort_registration: std::cell::Cell<Option<futures_util::future::AbortRegistration>>,
    event_source: std::cell::RefCell<Option<gloo_net::eventsource::futures::EventSource>>,
    attempt_count: std::cell::Cell<u32>,
    delay_ms: std::cell::Cell<u32>,
}
```

Note: `EventSource` is `!Send` (WASM-only), so we use `RefCell` not `Mutex`. `Cell` for simple u32 counters.

**`create()` function:**
1. Creates all signals (`is_reconnecting`, `last_error`) and `AbortHandle`.
2. Returns the `SseConnection` handle immediately. Does NOT open any connection.

**`connect_once()` method - internal retry loop (Option A):**

`connect_once()` contains its own retry loop with exponential backoff. It only returns when the EventSource is successfully established OR on permanent failure (cancelled / max_attempts exhausted). The consumer does NOT need to retry on transient failures.

1. Check `cancelled.get_untracked()` - if true, return `Err(SseError::Closed)`.
2. Enter retry loop:
   a. If `attempt_count > 0` (reconnection): set `is_reconnecting(true)`, wait `delay_ms`.
   b. **AbortHandle mechanism:** Before waiting, create a fresh `AbortHandle::new_pair()`. Store the handle in `self.abort_handle.set(new_handle)`. Wrap the wait in `Abortable::new(TimeoutFuture::new(delay_ms), registration)`. If `close()` was called during the wait, `abort_handle.abort()` was called, the `Abortable` returns `Err(Aborted)`, and we check `cancelled` to decide whether to break.
   c. **AbortRegistration lifecycle:** `AbortRegistration` is `!Clone` and consumed by `Abortable::new()`. We store it as `Cell<Option<AbortRegistration>>`. At the start of each wait, `.take()` the old registration (dropping it), create a fresh pair via `AbortHandle::new_pair()`, store the new handle, and use the new registration with `Abortable::new()`.
   d. Attempt `EventSource::new(&url)`. On failure: update `last_error`, increment `attempt_count`, if `max_attempts` reached return `Err(SseError::ConnectionFailed(...))`, else continue loop (retry).
3. On success: store `EventSource` in `RefCell` (replacing any old one), reset `is_reconnecting(false)`, `last_error(None)`, `delay_ms = initial_delay_ms`, `attempt_count = 0`. Return `Ok(())`.

**Consumer behavior:** `connect_once().await` blocks until connected or permanently failed. Consumer only sees `Ok(())` (connected) or `Err` (permanent). No consumer-side retry logic needed.

**`subscribe(channel)`:**
1. Mutably borrow `EventSource` from `RefCell` via `borrow_mut()`. If `None`, return `Err(SseError::ConnectionFailed("not connected"))`.
2. Call `es.subscribe(channel)` (takes `&mut self`) to get a `gloo_net` subscription stream.
3. Wrap in `SseStream` and return.

**`SseStream` implementation:**
- Holds the `gloo_net::eventsource::futures::EventSourceSubscription` stream.
- `Stream::poll` delegates to the underlying stream.
- Maps `Result<(String, MessageEvent), EventSourceError>` → `Result<SseEvent, SseError>`:
  - `Ok((event_type, msg))` → `Ok(SseEvent { event_type, data: msg.data().as_string().unwrap_or_default() })`
  - `Err(e)` → `Err(SseError::ConnectionFailed(e.to_string()))`

**`close()` method:**
1. Set `cancelled.set(true)`.
2. Call `self.abort_handle.get().abort()` - aborts any in-flight `Abortable` wait in `connect_once()`.
3. Take ownership of `EventSource` and close it: `if let Some(es) = self.event_source.borrow_mut().take() { es.close(); }` - `close()` consumes `self` so we must `.take()` from the RefCell.

**`AbortHandle` mechanism:** Uses `futures_util::future::{Abortable, AbortHandle}` (NOT `wasm_bindgen_futures::AbortHandle` which doesn't exist). Pattern:
```rust
let (abort_handle, abort_registration) = AbortHandle::new_pair();
// In connect_once():
let wait = Abortable::new(TimeoutFuture::new(delay_ms), abort_registration.clone());
wait.await; // Returns Err(Aborted) if abort_handle.abort() is called
```

**Consumer loop structure (for reference - implemented in Tasks 3-4):**
```rust
let mut conn = sse_stream::create(url, cancelled.clone(), config);
loop {
    if cancelled.get_untracked() { break; }
    match conn.connect_once().await {
        Ok(()) => {},
        Err(e) => { /* handle error, possibly break */ },
    }
    let log_stream = conn.subscribe("log")?;
    let status_stream = conn.subscribe("status")?;
    let result_stream = conn.subscribe("result")?;
    // ... select on streams, process events ...
    // When streams end → loop back for reconnection
}
```

**Steps:**
- [ ] Rename `crates/tama-web/src/utils.rs` to `crates/tama-web/src/utils/mod.rs` (pure rename, no content changes)
- [ ] Create `crates/tama-web/src/utils/sse_stream.rs` with the types and signatures above
- [ ] Implement `SseReconnectConfig` with `Default` (initial_delay_ms: 1000, max_delay_ms: 30000, max_attempts: None)
- [ ] Implement `SseError` enum with `Debug`, `Clone`, `Display` (3 variants: ConnectionFailed, SubscribeFailed, Closed)
- [ ] Implement `SseEvent` struct with `event_type: String` and `data: String` fields
- [ ] Implement `SseStream` wrapping the `gloo_net::eventsource::futures::EventSourceSubscription` stream - delegate `Stream::poll` to the underlying stream, mapping `Result<(String, MessageEvent), EventSourceError>` to `Result<SseEvent, SseError>`
- [ ] Implement `SseConnection` struct with fields: `url`, `config`, `cancelled`, `is_reconnecting`, `last_error`, `abort_handle: Cell<AbortHandle>`, `abort_registration: Cell<Option<AbortRegistration>>`, `event_source: RefCell<Option<EventSource>>`, `attempt_count: Cell<u32>`, `delay_ms: Cell<u32>`
- [ ] Implement `create()` — returns SseConnection without opening any connection. Creates initial `AbortHandle::new_pair()`, stores handle in Cell, registration in Cell<Option>
- [ ] Implement `connect_once()` — async method with INTERNAL retry loop: loops until EventSource::new() succeeds or cancelled/max_attempts. Uses exponential backoff between attempts. On each wait: takes old AbortRegistration from Cell, creates fresh pair, stores new handle, wraps TimeoutFuture in Abortable. Only returns Ok(()) on success or Err on permanent failure.
- [ ] Implement `subscribe()` - borrows EventSource from RefCell, calls es.subscribe(channel), wraps in SseStream
- [ ] Implement `close()` - sets cancelled, calls abort_handle.abort(), closes EventSource if present
- [ ] Implement `Drop` for `SseConnection` calling `close()`
- [ ] Run `cargo build --package tama-web --features ssr` (SSR build to catch server-side issues)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo build --package tama-web` (WASM build)
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat: extract SSE reconnection utility (utils/sse_stream.rs)"

**Acceptance criteria:**
- [ ] `utils.rs` renamed to `utils/mod.rs` with identical content
- [ ] `utils/sse_stream.rs` compiles with no warnings in WASM build (`cargo build --package tama-web`)
- [ ] `utils/sse_stream.rs` is gated with `#[cfg(not(feature = "ssr"))]` so SSR build is unaffected
- [ ] `SseConnection::close()` calls `abort_handle.abort()` for immediate cancellation
- [ ] `SseConnection` implements `Drop` calling `close()`
- [ ] `connect_once()` respects `max_attempts` - when reached, returns `Err(SseError::ConnectionFailed(...))`
- [ ] `connect_once()` uses exponential backoff: starts at `initial_delay_ms`, doubles each attempt, caps at `max_delay_ms`
- [ ] `subscribe()` returns `Err` when called before `connect_once()` (EventSource is None)
- [ ] All existing `crate::utils::*` imports still resolve (verified by build)
- [ ] `utils/self_update.rs` still compiles (imports from parent module via `use super::...` or `use crate::utils::...`)

**Note on testability:** The `sse_stream` module is WASM-only (uses `gloo_net`, `wasm_bindgen_futures`). Unit tests require `wasm-bindgen-test` which adds complexity. For this task, verification is via successful compilation of both WASM and SSR builds, plus clippy. Integration testing happens in Tasks 3-4 when consumers are refactored.

---

### Task 2: Create `ActivityPanel` component

**Context:**
`JobLogPanel` currently has a hardcoded UI: dark terminal styling, "Build logs" title, status badge, close button, and `<pre>` log rendering. This task creates a generic `ActivityPanel` shell that provides the header bar + scrollable container. Consumers pass their own content as children. This is purely presentational - no SSE logic, no data parsing.

**Files:**
- Create: `crates/tama-web/src/components/activity_panel.rs`
- Modify: `crates/tama-web/src/components/mod.rs`

**What to implement:**

Create `crates/tama-web/src/components/activity_panel.rs`:

```rust
//! ActivityPanel - generic UI shell for in-progress activity displays.
//!
//! Provides a header bar (title + status badge + optional close button) and
//! a scrollable body. Renders either a connection error, "Connecting..." empty
//! state, or the `children` content. No SSE logic - purely presentational.

use leptos::prelude::*;

/// ActivityPanel - a presentational shell for activity/progress displays.
///
/// Renders:
/// - Header bar with title, status badge, and optional close button
/// - Scrollable body showing: connection error (if any), empty state, or children
#[component]
pub fn ActivityPanel(
    /// Panel title displayed in the header bar.
    title: String,
    /// Current status - drives the status badge text (running/succeeded/failed/other).
    status: RwSignal<String>,
    /// Connection error. `Some(msg)` shows error in red, `None` shows normal content.
    connection_error: RwSignal<Option<String>>,
    /// Called when user clicks the close button. If `None`, no close button is shown.
    #[prop(optional)]
    on_close: Option<Callback<()>>,
    /// Child content rendered in the scrollable body when no error is present.
    children: Children,
) -> impl IntoView {
    let on_close_handler = move |_| {
        if let Some(cb) = on_close {
            cb.run(());
        }
    };

    view! {
        <div style="margin-top:1rem;border:1px solid var(--border,#ccc);border-radius:6px;background:#0f172a;color:#e2e8f0;font-family:monospace;font-size:0.75rem;max-height:300px;display:flex;flex-direction:column;">
            <div style="display:flex;justify-content:space-between;align-items:center;padding:0.5rem 0.75rem;background:#1e293b;border-bottom:1px solid #334155;">
                <div style="display:flex;align-items:center;gap:0.5rem;">
                    <span style="font-weight:600;">{title}</span>
                    <span style="font-size:0.75rem;color:#94a3b8;">
                        {move || {
                            let s = status.get();
                            match s.as_str() {
                                "running" => "● Running",
                                "succeeded" => "✓ Succeeded",
                                "failed" => "✗ Failed",
                                _ => "● Unknown",
                            }
                        }}
                    </span>
                </div>
                {move || {
                    if on_close.is_some() {
                        view! {
                            <button
                                type="button"
                                style="background:none;border:none;color:#94a3b8;cursor:pointer;font-size:1rem;"
                                on:click=on_close_handler
                            >
                                "×"
                            </button>
                        }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                }}
            </div>

            <div style="overflow-y:auto;padding:0.5rem 0.75rem;flex:1;">
                {move || {
                    if let Some(err) = connection_error.get() {
                        view! {
                            <div style="color:#ef4444;">{err}</div>
                        }.into_any()
                    } else {
                        (children)().into_any()
                    }
                }}
            </div>
        </div>
    }
}
```

**Key design decisions:**
- The dark terminal styling (background, font-family, font-size, max-height) stays on the outer `<div>`. This is the existing `JobLogPanel` styling - `ActivityPanel` inherits it as a reasonable default. Consumers can override with inline styles if needed, but the default matches the current look.
- Status badge mapping is identical to current `JobLogPanel`: `running` → "● Running", `succeeded` → "✓ Succeeded", `failed` → "✗ Failed", other → "● Unknown".
- Close button is only rendered when `on_close` is `Some`.
- When `connection_error` is `Some`, it replaces children entirely (error takes priority).
- When `connection_error` is `None` and children produce no content, the body is empty (no "Connecting..." text - that's a consumer concern).
- **For `pull_quant_wizard`:** The wizard's download job cards (progress bars, status badges) are wrapped in `ActivityPanel`. The panel provides the header bar and scrollable container. The wizard's children content keeps its own styling - the dark terminal background is a reasonable container for progress bars. If the visual result is undesirable, the wizard can override the outer div's background with an inline style prop in a follow-up task (out of scope for this plan).

**Steps:**
- [ ] Create `crates/tama-web/src/components/activity_panel.rs` with the component above
- [ ] Add `pub mod activity_panel;` to `crates/tama-web/src/components/mod.rs`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat: add ActivityPanel shared UI shell component"

**Acceptance criteria:**
- [ ] `ActivityPanel` compiles with no warnings
- [ ] Status badge renders correctly for "running", "succeeded", "failed", and unknown values
- [ ] Close button only appears when `on_close` is `Some`
- [ ] Connection error replaces children when present
- [ ] Component uses inline styles matching current `JobLogPanel` dark terminal look

---

### Task 3: Refactor `JobLogPanel` to use `ActivityPanel` + `sse_stream`

**Context:**
`JobLogPanel` currently owns its own SSE reconnection loop (~120 lines) and its own UI rendering. This task refactors it to delegate the SSE connection to `sse_stream::connect()` and the UI rendering to `ActivityPanel`. The public API (`job_id`, `on_close`, `on_result`, `on_status`) stays identical so all 4 callers are unaffected.

**Files:**
- Modify: `crates/tama-web/src/components/job_log_panel.rs`

**What to implement:**

Replace the current `JobLogPanel` implementation with one that uses `sse_stream::create()` + consumer-driven loop + `ActivityPanel`.

**Complete refactored structure (this is the target code):**

```rust
use crate::components::activity_panel::ActivityPanel;
use crate::utils::sse_stream::{self, SseConnection};
use futures_util::StreamExt;
use leptos::prelude::*;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct LogPayload { line: String }
#[derive(Debug, Clone, Deserialize)]
struct StatusPayload { status: String }
#[derive(Debug, Clone, Deserialize)]
struct ResultPayload { results: String }

#[component]
pub fn JobLogPanel(
    job_id: String,
    #[prop(optional)] on_close: Option<Callback<()>>,
    #[prop(optional)] on_result: Option<Callback<String>>,
    #[prop(optional)] on_status: Option<Callback<String>>,
) -> impl IntoView {
    let lines = RwSignal::new(Vec::<String>::new());
    let status = RwSignal::new(String::from("running"));
    let cancelled = RwSignal::new(false);

    on_cleanup(move || {
        cancelled.set(true);
    });

    let job_id_for_effect = job_id.clone();
    Effect::new(move |_| {
        let job_id = job_id_for_effect.clone();
        if job_id.is_empty() {
            return;
        }

        let url = format!("/tama/v1/backends/jobs/{job_id}/events");
        let mut conn = sse_stream::create(url, cancelled.clone(), None);

        wasm_bindgen_futures::spawn_local(async move {
            loop {
                if cancelled.get_untracked() {
                    break;
                }

                // Connect (or reconnect) with exponential backoff
                match conn.connect_once().await {
                    Ok(()) => {},
                    Err(e) => {
                        // For infinite retry (None config), connect_once only
                        // returns Err on cancellation or max_attempts.
                        // Since we use None (infinite), this only happens on cancel.
                        break;
                    }
                }

                // Subscribe to channels
                let mut log_stream = match conn.subscribe("log") {
                    Ok(s) => s,
                    Err(_) => { continue; } // connection dropped, loop back
                };
                let mut status_stream = match conn.subscribe("status") {
                    Ok(s) => s,
                    Err(_) => { continue; }
                };
                let mut result_stream = match conn.subscribe("result") {
                    Ok(s) => s,
                    Err(_) => { continue; }
                };

                // Inner event processing loop - same select pattern as current code
                loop {
                    if cancelled.get_untracked() {
                        break;
                    }

                    // Create next() futures FIRST, then pin_mut, then select.
                    // Do NOT call .next() twice - that's a double mutable borrow.
                    let next_log = log_stream.next();
                    let next_status = status_stream.next();
                    let next_result = result_stream.next();
                    futures_util::pin_mut!(next_log, next_status, next_result);
                    let first = futures_util::future::select(next_log, next_status);
                    match futures_util::future::select(first, next_result).await {
                        futures_util::future::Either::Left((inner, _)) => {
                            match inner {
                                futures_util::future::Either::Left((Some(Ok(event)), _)) => {
                                    // Log event
                                    if let Ok(payload) = serde_json::from_str::<LogPayload>(&event.data) {
                                        lines.update(|v| {
                                            v.push(payload.line);
                                            if v.len() > 1000 {
                                                v.drain(0..v.len()-1000);
                                            }
                                        });
                                    }
                                }
                                futures_util::future::Either::Right((Some(Ok(event)), _)) => {
                                    // Status event
                                    if let Ok(payload) = serde_json::from_str::<StatusPayload>(&event.data) {
                                        status.set(payload.status.clone());
                                        if let Some(cb) = on_status.as_ref() {
                                            cb.run(payload.status.clone());
                                        }
                                        if payload.status != "running" {
                                            break; // terminal status - exit inner loop
                                        }
                                    }
                                }
                                _ => { break; } // stream ended, loop back for reconnect
                            }
                        }
                        futures_util::future::Either::Right((Some(Ok(event)), _)) => {
                            // Result event
                            if let Ok(payload) = serde_json::from_str::<ResultPayload>(&event.data) {
                                if let Some(cb) = on_result.as_ref() {
                                    cb.run(payload.results);
                                }
                            }
                        }
                        _ => { break; } // stream ended
                    }
                }
                // Inner loop exited (terminal status or stream end) → outer loop reconnects
            }
        });
    });

    view! {
        <ActivityPanel
            title="Build logs"
            status=status
            connection_error=/* see note below */
            on_close=on_close
        >
            {move || {
                let all_lines = lines.get();
                if all_lines.is_empty() {
                    view! {
                        <div style="color:#94a3b8;">"Connecting..."</div>
                    }.into_any()
                } else {
                    view! {
                        <pre style="margin:0;white-space:pre-wrap;word-break:break-all;">
                            {all_lines.join("\n")}
                        </pre>
                    }.into_any()
                }
            }}
        </ActivityPanel>
    }
}
```

**Note on `connection_error`:** The `conn.last_error()` returns an `RwSignal<Option<String>>`. However, `conn` is created inside `Effect::new` and moved into `spawn_local`, so it's not accessible from the `view!` block. **Solution:** Create a separate `connection_error: RwSignal<Option<String>>` signal BEFORE the Effect, clone it into the async block, and have the async block update it when `conn.last_error()` changes. Alternatively, the async block can directly set the `connection_error` signal on connect failures.

**What to preserve from current implementation:**
- SSE endpoint URL: `/tama/v1/backends/jobs/{job_id}/events`
- Channel names: "log", "status", "result"
- Payload parsing: `LogPayload { line }`, `StatusPayload { status }`, `ResultPayload { results }`
- 1000-line cap with `drain(0..drop_count)`
- `futures_util::future::select` multiplexing pattern for 3 streams
- Terminal status detection: when status != "running", break inner loop
- `on_status` and `on_result` callbacks

**What changes:**
- No more manual `EventSource::new()`, exponential backoff, or `is_reconnecting` flag - all handled by `conn.connect_once()`
- No more manual UI rendering - delegated to `ActivityPanel`
- Consumer owns the reconnection loop (calls `connect_once()` → `subscribe()` → process → loop back)

**Steps:**
- [ ] Read the current `job_log_panel.rs` to understand the exact select pattern and payload parsing
- [ ] Replace the component with the target code structure above: `sse_stream::create()` + consumer loop + `ActivityPanel`
- [ ] Create `connection_error: RwSignal<Option<String>>` BEFORE the Effect, clone into async block, update on connect failures
- [ ] Keep the `futures_util::future::select` multiplexing for 3 channels (log, status, result)
- [ ] Keep the 1000-line cap logic
- [ ] Keep the `on_status` and `on_result` callbacks
- [ ] Keep terminal status detection: when status != "running", break inner loop
- [ ] Remove the manual `EventSource::new()`, backoff loop, `is_reconnecting`, `delay_ms`, and `MAX_DELAY_MS` variables
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Verify visually: open the web UI, trigger a backend job (install/update), confirm the JobLogPanel renders identically to before (dark terminal, "Build logs" title, status badge, close button, scrolling logs)
- [ ] Commit with message: "refactor: JobLogPanel uses ActivityPanel + sse_stream"

**Acceptance criteria:**
- [ ] `JobLogPanel` public API is identical (same props: `job_id`, `on_close`, `on_result`, `on_status`)
- [ ] All 4 callers compile without changes: `benchmarks/mod.rs`, `benchmarks/spec_bench.rs`, `updates.rs`, `backends.rs`
- [ ] SSE endpoint URL unchanged: `/tama/v1/backends/jobs/{job_id}/events`
- [ ] 1000-line cap preserved
- [ ] UI renders identically to pre-refactor (dark terminal, header bar, status badge, close button)
- [ ] No manual reconnection code remains in `job_log_panel.rs`

---

### Task 4: Refactor `pull_quant_wizard` to use `sse_stream` + `ActivityPanel`

**Context:**
`pull_quant_wizard.rs` has ~150 lines of inline SSE reconnection logic that duplicates the pattern in `JobLogPanel`. It spawns one SSE connection per download job (N connections for N jobs), each with capped retries (max 10 attempts). This task replaces the inline loop with `sse_stream::connect()` and wraps the download job list in `ActivityPanel`.

**Files:**
- Modify: `crates/tama-web/src/components/pull_quant_wizard.rs`

**What to implement:**

1. **Replace the SSE reconnection loop** (lines ~384-490) with `sse_stream::create()` + consumer loop:
   - For each job, create a connection: `sse_stream::create(url, job_cancelled, Some(config))`
   - Config: `SseReconnectConfig { max_attempts: Some(10), ..Default::default() }` - matches current `MAX_RECONNECT_ATTEMPTS = 10`
   - Consumer loop: `conn.connect_once().await` → `conn.subscribe("progress")` / `conn.subscribe("done")` → process events → loop back
   - When `max_attempts` is exhausted, `connect_once()` returns `Err` - the wizard's event handler marks the job as "failed" (same as current `j.status = "failed"`)

2. **Wrap the download job list in `ActivityPanel`**:
   - The download step currently renders a list of download job cards (progress bars, status badges, cancel buttons)
   - Wrap this content in `<ActivityPanel title="Downloading" status=overall_status connection_error=overall_error >`
   - `overall_status` is a derived signal: "running" if any job is downloading, "succeeded" if all completed, "failed" if any failed
   - `overall_error` is `None` (per-job errors are shown inline on each job card)

3. **Preserve existing behavior:**
   - Per-job state updates (status, bytes_downloaded, error messages)
   - `advance_if_all_terminal()` callback - called when a job reaches terminal state
   - Per-job progress bar rendering
   - Cancel button per job

**What changes:**
- No more manual `EventSource::new()`, exponential backoff, `reconnect_attempts` counter, or `delay_ms` in the wizard
- Download job list is wrapped in `ActivityPanel` header + scrollable container
- `MAX_RECONNECT_ATTEMPTS` constant removed (now passed as `SseReconnectConfig::max_attempts`)

**Critical preservation - `advance_if_all_terminal()`:**
The current wizard calls `advance_if_all_terminal()` from within the SSE event processing loop when a job reaches terminal state. In the refactored version:
- The `advance_if_all_terminal()` call stays in the wizard's event processing closure (the closure passed to the stream handler)
- It is NOT moved into `sse_stream` - `sse_stream` knows nothing about wizard state
- When `sse_stream::connect()` exits due to `max_attempts` exhaustion, the wizard's event handler detects the stream ended and calls `advance_if_all_terminal()`
- The overall behavior must be identical: wizard advances to next step when ALL jobs reach terminal state (completed or failed)

**What stays the same:**
- Per-job download state model (status, bytes, error)
- `advance_if_all_terminal()` logic
- Progress bar rendering
- Cancel button behavior

**Steps:**
- [ ] Read the current `pull_quant_wizard.rs` SSE section (lines ~384-490) to understand the exact channel names and event handling
- [ ] Replace the per-job SSE loop with `sse_stream::connect(url, cancelled, Some(config))` where config has `max_attempts: Some(10)`
- [ ] Replace `es.subscribe("progress")` and `es.subscribe("done")` with `conn.subscribe("progress")` and `conn.subscribe("done")`
- [ ] When the connection loop exits (max attempts reached), mark the job as "failed" - same as current behavior
- [ ] Add `overall_status` and `overall_error` signals for `ActivityPanel`
- [ ] Wrap the download job list in `<ActivityPanel>` component
- [ ] Keep `advance_if_all_terminal()` in the event processing closure
- [ ] Remove `MAX_RECONNECT_ATTEMPTS` constant, `reconnect_attempts` variable, and `delay_ms` variable
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Verify visually: open the web UI, trigger a model pull, confirm the download wizard renders correctly with ActivityPanel header
- [ ] Commit with message: "refactor: pull_quant_wizard uses sse_stream + ActivityPanel"

**Acceptance criteria:**
- [ ] No manual SSE reconnection code remains in `pull_quant_wizard.rs`
- [ ] `max_attempts: Some(10)` preserved (behavior identical to pre-refactor)
- [ ] Per-job progress bars, status badges, and cancel buttons render correctly
- [ ] `advance_if_all_terminal()` still called on terminal state
- [ ] Download job list wrapped in `ActivityPanel` with "Downloading" title
- [ ] When max attempts exhausted, job is marked "failed" (same as before)

---

### Task 5: Update `jobs.rs` comment and final verification

**Context:**
The reviewer noted that `jobs.rs` line 287 has a comment referencing `JobLogPanel`. Since `JobLogPanel`'s internal implementation changed but its external contract (SSE event handling) is the same, the comment should reference the SSE event contract, not the component specifically. This task also runs the full workspace check to ensure everything compiles and passes CI.

**Files:**
- Modify: `crates/tama-web/src/jobs.rs`

**What to implement:**

1. Update the comment in `jobs.rs` (around line 287) from referencing `JobLogPanel` to referencing the SSE event contract:
   - Current: "The JobLogPanel component waits for this event..."
   - Change to: "Clients subscribing to job SSE events wait for this terminal status event..."

2. Run the full workspace check:
   - `cargo build --workspace`
   - `cargo build --release --workspace`
   - `cargo clippy --workspace -- -D warnings`
   - `cargo test --workspace`
   - `cargo fmt --all`

**Steps:**
- [ ] Update the comment in `crates/tama-web/src/jobs.rs` to reference the SSE event contract instead of `JobLogPanel` specifically
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo build --release --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo test --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "chore: update jobs.rs comment + verify workspace build"

**Acceptance criteria:**
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo build --release --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` is clean
- [ ] `cargo test --workspace` passes
- [ ] `cargo fmt --all` is clean
- [ ] `jobs.rs` comment references SSE event contract, not `JobLogPanel`

---

## Summary

| Task | File(s) | Type | Lines Changed (approx) |
|------|---------|------|---|
| 1. SSE utility + utils module split | `utils/mod.rs`, `utils/sse_stream.rs` | New | ~150 new |
| 2. ActivityPanel component | `components/activity_panel.rs`, `components/mod.rs` | New | ~60 new |
| 3. JobLogPanel refactor | `components/job_log_panel.rs` | Refactor | -120 (SSE loop), +30 (new) |
| 4. pull_quant_wizard refactor | `components/pull_quant_wizard.rs` | Refactor | -150 (SSE loop), +20 (new) |
| 5. Comment + verification | `jobs.rs` | Comment | ~2 |

**Net effect:** ~200 lines of duplicated SSE code eliminated, replaced by a shared ~150-line utility. No breaking changes to any consumer APIs.

**Known out-of-scope consumer:** `utils/self_update.rs` also uses SSE (`EventSource::new("/tama/v1/self-update/events")`, subscribes to "log" and "status" channels, uses `select`). It does NOT have reconnection logic (one-shot connection), so it's a simpler case. It could benefit from `SseConnection::subscribe()` in a follow-up task to eliminate the manual `EventSource::new()` + `subscribe()` + `select` dance, but this is explicitly out of scope for this plan.
