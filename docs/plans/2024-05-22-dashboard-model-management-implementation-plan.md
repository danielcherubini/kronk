# Implementation Plan: Interactive Model Management on Dashboard

This plan follows the technical specification to surface per-model status via the SSE stream and enable inline Load/Unload actions on the Dashboard.

## Phase 1: Backend Testing (`koji-core`)

**Goal:** Ensure the `collect_model_statuses` logic is robust and the `models` field is correctly broadcast.

### Task 1: `collect_model_statuses` Unit Tests
**File:** `crates/koji-core/src/proxy/status.rs`
- **Test 1: Idle State:** Verify that if no models are in `ModelState::Ready`, `collect_model_statuses` returns all configured models with `loaded: false`.
- **Test 2: Loaded State:** Verify that if a model's server is in `ModelState::Ready`, it is correctly reported as `loaded: true`.
- **Test 3: Non-Ready States:** Verify that `ModelState::Starting` or `ModelState::Failed` are NOT treated as `loaded`.
- **Test 4: Sorting:** Verify that the returned vector is always sorted by `id`.

### Task 2: Metrics Broadcast & SSE Integration Tests
**File:** `crates/koji-core/src/proxy/server/mod.rs`
- **Test 1: Field Presence:** Extend the existing broadcast test to assert that the `models` array is present in the `MetricSample` sent over the channel.
- **Test 2: SSE Round-trip:** Extend the SSE stream test to ensure the JSON payload correctly deserializes the `models` field into the expected `Vec<ModelStatus>`.

---

## Phase 2: Frontend Implementation (`koji-web`)

**Goal:** Transform the Dashboard from a static overview to an interactive control center.

### Task 3: Data Model Alignment
**File:** `crates/koji-web/src/pages/dashboard.rs`
- Define the `ModelStatus` struct to match the backend.
- Update `MetricSample` with `#[serde(default)] pub models: Vec<ModelStatus>`.

### Task 4: UI Overhaul - "Active Models" Section
**File:** `crates/koji-web/src/pages/dashboard.rs`
- **Remove:** The current single-value "Models Loaded" card.
- **Add:** A new section below the stats grid.
- **Empty State:** Show a "No models configured" message if `h.models` is empty.
- **Grid View:** If models exist, render a `.models-grid` containing `model-card` elements.

### Task 5: Interactive Model Rows & Actions
**File:** `crates/koji-web/src/pages/dashboard.rs`
- **Visuals:** Each card will display the Model ID, the Backend name, and a Status Badge (`Loaded` in green, `Idle` in gray).
- **Actions:** 
    - Use `leptos::Action` to implement `load_action` and `unload_action`.
    - Buttons will be injected into a `.model-card__actions` container.
    - **Safety:** Use `.pending()` on the actions to disable buttons during flight, preventing double-clicks.
- **Real-time:** No manual refresh logic is required; the existing SSE `Effect` will naturally trigger a re-render when the new `models` data arrives.

### Task 6: Styling
**File:** `crates/koji-web/style.css`
- Add `.dashboard-models` and `.dashboard-models .page-header` spacing rules to maintain layout consistency.

---

## Verification Checklist
1. [ ] `cargo fmt --all`
2. [ ] `cargo clippy --workspace -- -D warnings`
3. [ ] `cargo test --package koji-core` (All new unit tests pass)
4. [ ] `trunk build` (Frontend compiles without errors)
5. [ ] **Manual Smoke Test:**
    - Load a model via `/models` page.
    - Observe the Dashboard update automatically within 2 seconds.
    - Click "Unload" on the Dashboard and verify the status badge changes immediately.
