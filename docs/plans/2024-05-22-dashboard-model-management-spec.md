# Specification: Interactive Model Management on Dashboard

## Overview
This feature adds the ability to view and control model loading/unloading directly from the performance dashboard. Instead of just showing a count of loaded models, the dashboard will display a real-time list of models and their current status, with buttons to trigger load/unload actions.

## Goals
- Provide a real-time view of active models on the Dashboard.
- Enable quick actions (Load/Unload) without navigating away from the dashboard.
- Ensure the UI stays in sync with the system state via the existing metrics stream.

## Technical Changes

### 1. Backend (koji-core)
The `MetricSample` sent via the `/koji/v1/system/metrics/stream` endpoint must be expanded to include model information.

**New Data Structure (in `MetricSample`):**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    pub id: String,
    pub loaded: bool,
    // Other minimal info if needed (e.g., backend)
}

// Update existing MetricSample
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetricSample {
    // ... existing fields ...
    pub models_loaded: u64, // existing
    pub models: Vec<ModelStatus>, // NEW: list of models with their status
}
```

**Implementation Steps:**
- Modify the metrics collection loop to iterate through the `ProxyState.models` map.
- For each model, determine if it is "loaded" (e.g., based on its `ModelState`).
- Include this list in the `MetricSample` sent via the broadcast channel.

### 2. Frontend (koji-web)

#### Data Model Update
Update `MetricSample` in `crates/koji-web/src/pages/dashboard.rs` to include the `models` field.

#### UI Update
Modify the `Dashboard` component in `crates/koji-web/src/pages/dashboard.rs`:
- Replace the simple `models_loaded` card with a more descriptive "Active Models" section.
- Implement a new sub-component or view block that iterates over `h.models`.
- For each model, render:
    - A row/card containing the Model ID.
    - A status badge (`Loaded` vs `Idle`).
    - A button:
        - If `loaded == true` -> `Unload` button (triggers `unload_action`).
        - If `loaded == false` -> `Load` button (triggers `load_action`).

#### Action Integration
Reuse the `load_action` and `unload_action` logic (currently in `models.rs`) or implement them within `dashboard.rs` to ensure the buttons actually trigger the API calls.

## Testing Plan
1. **Unit Tests (Backend):** Verify that the metrics collection correctly identifies model states and populates the `models` list.
2. **Integration Test (Frontend):**
    - Verify that the Dashboard displays the correct number of models.
    - Verify that clicking "Load" on an idle model triggers the correct API call.
    - Verify that clicking "Unload" on a loaded model triggers the correct API call.
    - Verify that the status badge updates automatically when the model state changes via the stream.

## Implementation Notes
- Use `leptos_router::components::A` for navigation if needed, but the goal is to use `Action` for the buttons to avoid page reloads.
- Ensure the CSS for the new model list/cards is consistent with the existing `model-card` style used in `models.rs`.
