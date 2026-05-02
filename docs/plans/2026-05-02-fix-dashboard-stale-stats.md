# Fix Dashboard Stale Stats After Browser Idle

**Goal:** Prevent the dashboard from showing stale CPU/memory/GPU/VRAM stats when the user returns to a backgrounded browser tab.
**Architecture:** Add three triggers (SSE `lagged` event, `visibilitychange`, SSE reconnect) that all call a shared backfill function to fetch recent metrics from `/metrics/history` and merge them into the buffer.
**Tech Stack:** Leptos (Rust/WASM), `web_sys::EventSource`, `gloo_net`, browser `visibilitychange` API

---

## Context

When the browser tab is backgrounded, the `EventSource` (SSE) connection is throttled or disconnected by the browser. All metric samples during the idle period are lost. When focus returns, only the next SSE `sample` event arrives — the dashboard shows stale values from when the tab was hidden until the user manually refreshes.

The backend already supports everything needed:
- SSE `lagged` event with `{"missed": N}` (broadcast channel subscriber lag)
- `GET /tama/v1/system/metrics/history?limit=N` (SQLite backfill, up to 1000)

Both mechanisms are currently **unused by the frontend**. This plan wires them together.

---

### Task 1: Add shared backfill function and merge helper

**Context:**
Both the SSE `lagged` handler and the `visibilitychange` handler need to fetch history and merge it into the `history` buffer. Extracting this into a shared async function avoids code duplication and ensures both paths behave identically. The merge logic must deduplicate by timestamp (keeping the latest sample) and trim to 450 entries.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard.rs`
- Test: `crates/tama-web/src/pages/dashboard.rs` (add tests in existing `#[cfg(test)] mod tests`)

**What to implement:**

1. **`merge_samples` free function** — a pure function for merging new samples into an existing buffer:

```rust
/// Merge new metric samples into the buffer.
/// Combines, sorts by timestamp, deduplicates (keeping the FIRST entry for each timestamp),
/// and trims to the last `max_len` samples.
///
/// Keeping the first entry is intentional: SSE entries (which include `models` data)
/// are already in the buffer, and backfill entries (which have `models: vec![]`)
/// are extended after. Keeping the first preserves the richer SSE entry.
fn merge_samples(buf: &mut Vec<MetricSample>, new: Vec<MetricSample>, max_len: usize) {
    buf.extend(new);
    buf.sort_by_key(|s| s.ts_unix_ms);
    buf.dedup_by(|a, b| a.ts_unix_ms == b.ts_unix_ms); // keeps a (first), removes b (subsequent)
    if buf.len() > max_len {
        buf.drain(..buf.len() - max_len);
    }
}
```

2. **`backfill_metrics` async function** — fetches history and merges via `merge_samples`:

```rust
async fn backfill_metrics(
    history: RwSignal<Vec<MetricSample>>,
    last_backfill: RwSignal<u64>,
) {
    // Cooldown: skip if backfilled in the last 5 seconds
    let now = js_sys::Date::now() as u64;
    if (now - last_backfill.get()) < 5000 {
        return;
    }
    last_backfill.set(now);

    let url = "/tama/v1/system/metrics/history?limit=200";
    match gloo_net::http::Request::get(url).send().await {
        Ok(resp) => {
            let _ = extract_and_store_csrf_token(&resp);
            match resp.json::<Vec<MetricsHistoryEntry>>().await {
                Ok(entries) => {
                    let new: Vec<MetricSample> = entries.into_iter().map(Into::into).collect();
                    if !new.is_empty() {
                        history.update(|buf| {
                            merge_samples(buf, new, 450);
                        });
                    }
                }
                Err(e) => log::warn!("backfill: failed to parse history JSON: {}", e),
            }
        }
        Err(e) => log::warn!("backfill: failed to fetch /metrics/history: {}", e),
    }
}
```

Both functions must be placed **outside** the `Dashboard` component (as module-level free functions) so `merge_samples` can be unit-tested.

3. **Unit tests for `merge_samples`** in the existing `#[cfg(test)] mod tests`:

- `test_merge_samples_combines_two_buffers` — two non-overlapping buffers merge and sort correctly
- `test_merge_samples_dedupes_by_timestamp_keeps_first` — overlapping timestamps keep the first entry (the SSE entry with `models` data)
- `test_merge_samples_trims_to_max_len` — buffer exceeding `max_len` is trimmed from the front
- `test_merge_samples_empty_new_does_nothing` — empty `new` leaves buffer unchanged
- `test_merge_samples_empty_buf_populates_from_new` — empty buffer gets populated
- `test_merge_samples_all_timestamps_overlap_keeps_existing` — when `new` has the same timestamps as `buf` but different data values, the existing (first) entries survive dedup

**Steps:**
- [ ] Write the 5 failing tests for `merge_samples` in `dashboard.rs`
- [ ] Run `cargo test --package tama-web -- dashboard::tests::test_merge` — verify all fail
- [ ] Implement `merge_samples` function
- [ ] Run `cargo test --package tama-web -- dashboard::tests::test_merge` — verify all pass
- [ ] Implement `backfill_metrics` async function (cannot unit-test due to WASM deps — tested manually)
- [ ] Run `cargo build --package tama-web` — verify it compiles
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Commit with message: "feat: add shared backfill function and merge helper for dashboard metrics"

**Acceptance criteria:**
- [ ] All 6 `merge_samples` tests pass
- [ ] `merge_samples` correctly deduplicates by keeping the **first** entry (SSE entry with `models` data preserved)
- [ ] `backfill_metrics` compiles and calls the history endpoint with `limit=200`
- [ ] No clippy warnings

---

### Task 2: Wire SSE lagged handler, visibility change, and SSE reconnect

**Context:**
With the shared `backfill_metrics` function in place, wire it to three triggers that cover all the scenarios where the SSE stream misses data: broadcast channel lag, browser tab backgrounding, and SSE disconnection/reconnection.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard.rs`

**What to implement:**

1. **Add two new signals** in `Dashboard` (near `history`, `fetch_failed`, `connect_trigger`):

```rust
let last_backfill = RwSignal::new(0u64);
let reconnect_pending = RwSignal::new(false);
```

2. **Add SSE `lagged` event handler** — add a new event listener on the `EventSource` alongside the existing `sample` and `error` listeners:

```rust
let on_lagged = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
    if let Some(data_str) = evt.data().as_string() {
        // Parse {"missed": N} to log, then backfill
        log::info!("SSE lagged event received: {}", data_str);
        let history_copy = history;
        let last_backfill_copy = last_backfill;
        spawn_local(backfill_metrics(history_copy, last_backfill_copy));
    }
});
let _ = es.add_event_listener_with_callback("lagged", on_lagged.as_ref().unchecked_ref());
on_lagged.forget();
```

3. **Add SSE reconnect detection** — modify the existing `sample` handler to detect reconnection and trigger backfill. Wrap the `sample` handler logic to check `reconnect_pending`:

After `fetch_failed.set(false)` in the sample handler, add:

```rust
if reconnect_pending.get() {
    reconnect_pending.set(false);
    log::info!("SSE reconnected, backfilling metrics");
    let history_copy = history;
    let last_backfill_copy = last_backfill;
    spawn_local(backfill_metrics(history_copy, last_backfill_copy));
}
```

And modify the existing `error` handler to set the flag:

```rust
let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
    fetch_failed.set(true);
    reconnect_pending.set(true);
});
```

5. **Reset `reconnect_pending` on manual retry** — add to the existing `manual_refresh` handler:

```rust
let manual_refresh = move |_| {
    fetch_failed.set(false);
    reconnect_pending.set(false); // reset so next sample doesn't trigger a spurious backfill
    connect_trigger.update(|n| *n += 1);
};
```

6. **Set `last_backfill` after initial history fetch** — after the initial `spawn_local` block that fetches 450 history entries succeeds, set the cooldown:

```rust
// Inside the initial history fetch spawn_local:
if !samples.is_empty() {
    history_signal.update(|buf| {
        *buf = samples;
    });
    last_backfill.set(js_sys::Date::now() as u64); // prevent immediate redundant backfill
}
```

This prevents the first `lagged` or `visibilitychange` event from firing a redundant backfill immediately after mount.

4. **Add `visibilitychange` listener** — after the `Effect::new` block that opens the SSE connection, add a separate `Effect` for visibility:

```rust
Effect::new(move |_| {
    let history_sig = history;
    let last_backfill_sig = last_backfill;
    let on_visibility = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
        if web_sys::document().and_then(|d| d.visibility_state().ok()) == Some("visible".to_string()) {
            spawn_local(backfill_metrics(history_sig, last_backfill_sig));
        }
    });
    // Clone the JS function reference (cheap, not the Closure itself) for both add and remove
    let js_fn: js_sys::Function = on_visibility.as_ref().unchecked_ref::<js_sys::Function>().clone();
    let doc = web_sys::document().expect("document");
    let _ = doc.add_event_listener_with_callback("visibilitychange", &js_fn);

    // on_cleanup owns on_visibility — it stays alive until cleanup runs
    on_cleanup(move || {
        let _ = doc.remove_event_listener_with_callback("visibilitychange", &js_fn);
        // on_visibility dropped here — cleans up WASM closure memory
    });
});
```

**Note:** Do NOT use `.clone()` on the `Closure` (it doesn't implement `Clone`). Do NOT use `.forget()` (prevents cleanup). Instead, clone the `js_sys::Function` reference (which is cheap) and let `on_cleanup` own the `Closure`.

**Steps:**
- [ ] Add `last_backfill` and `reconnect_pending` signals
- [ ] Add SSE `lagged` event listener
- [ ] Modify SSE `sample` handler to check `reconnect_pending` and trigger backfill
- [ ] Modify SSE `error` handler to set `reconnect_pending = true`
- [ ] Add `reconnect_pending.set(false)` to the `manual_refresh` handler
- [ ] Set `last_backfill` after the initial history fetch succeeds (prevents immediate redundant backfill)
- [ ] Add `visibilitychange` listener with proper cleanup (using `js_sys::Function` clone, NOT `Closure::clone()`)
- [ ] Run `cargo build --package tama-web` — verify it compiles
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Commit with message: "feat: wire SSE lagged, visibility change, and reconnect triggers for dashboard metrics backfill"

**Acceptance criteria:**
- [ ] SSE `lagged` event triggers `backfill_metrics` via `spawn_local`
- [ ] SSE `error` sets `reconnect_pending = true`; first `sample` after error resets it and triggers backfill
- [ ] Manual retry (`manual_refresh`) resets `reconnect_pending` to avoid spurious backfill
- [ ] After initial history fetch, `last_backfill` is set to current time
- [ ] `visibilitychange` to `"visible"` triggers `backfill_metrics` (with shared 5s cooldown)
- [ ] The `visibilitychange` listener is properly cleaned up when the component unmounts (uses `js_sys::Function` clone, no `.forget()`)
- [ ] No clippy warnings

---

### Task 3: Manual verification

**Context:**
The backfill logic is difficult to unit-test in WASM (it depends on `spawn_local`, `gloo_net`, and `js_sys`). Manual verification ensures the feature works end-to-end in a real browser.

**Files:**
- No file changes

**What to implement:**
Nothing — this is a verification task only.

**Steps:**
- [ ] Run `cargo build --release --workspace` — verify full release build
- [ ] Start tama server, open dashboard in browser
- [ ] Background the tab (switch away) for ~30 seconds
- [ ] Return to the tab — verify stats are current (not stale)
- [ ] Open browser DevTools console — verify no errors on `visibilitychange`
- [ ] Verify the `log::warn!` output appears when backfill is attempted
- [ ] Verify the `log::info!("SSE reconnected...")` message appears when returning to tab
- [ ] Commit with message: "chore: verify dashboard stale stats fix works in browser"

**Acceptance criteria:**
- [ ] Returning to a backgrounded tab shows current (not stale) stats
- [ ] No console errors
- [ ] Sparkline charts show continuous data (no gaps) after returning from background

---

## Summary

| Task | Description | File(s) |
|------|-------------|---------|
| 1 | Shared backfill function + merge helper + unit tests | `dashboard.rs` |
| 2 | Wire SSE lagged, visibility change, reconnect triggers | `dashboard.rs` |
| 3 | Manual verification | none |

**No backend changes required.**
