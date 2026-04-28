# Dashboard Filter Loaded Models Plan

**Goal:** Show only loaded (ready) models in the dashboard's "Active Models" section instead of all configured models, while preserving correct UX for empty states.

**Architecture:** Extract a new testable helper function `loaded_models()` that filters a `&[ModelStatus]` to only ready entries. Use it in two places: (1) the models list passed to the render loop, and (2) the "X loaded" count heading. Keep the unfiltered list for the empty-state check so the UI can distinguish "no models configured" from "no models loaded." This is a frontend-only change — no backend API changes.

**Tech Stack:** Leptos (Rust/WASM), serde, existing `ModelStatus` struct.

---

### Task 1: Add `loaded_models()` helper, filter dashboard models, fix empty-state UX

**Context:**
The dashboard's "Active Models" section currently renders every configured model regardless of its lifecycle state. The user wants to see only models that are currently loaded (state == "ready"). The `loaded_model_count()` helper already counts ready models, and the `ModelStatus.state` field carries the lifecycle state ("ready", "idle", "loading", "unloading", "failed").

We need three changes:
1. **Extract a `loaded_models()` helper** — a pure function that filters `&[ModelStatus]` to ready entries. This enables TDD and centralizes the filter predicate.
2. **Filter the render list** — only pass ready models to the ModelRow render loop.
3. **Fix empty-state UX** — after filtering, `models.is_empty()` would be true when models exist but none are loaded. We must keep the unfiltered list for the empty-state check so the UI distinguishes "no models configured yet" (no models at all) from "No models currently loaded" (models exist but none are ready).

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard.rs`

**What to implement:**

#### Step A: Add `loaded_models()` helper function

Add a new helper function near the existing `loaded_model_count()` helper (around line 125):

```rust
/// Filter models to only those that are currently loaded (state == "ready").
///
/// Used by the dashboard to render the Active Models list and by the
/// "X loaded" summary heading. Extracted as a free function so it can
/// be unit-tested independently of the Leptos reactive view.
fn loaded_models(models: &[ModelStatus]) -> Vec<ModelStatus> {
    models.iter().filter(|m| m.state == "ready").cloned().collect()
}
```

#### Step B: Write tests for `loaded_models()`

Add unit tests in the `#[cfg(test)]` module at the bottom of `dashboard.rs`:

```rust
    /// `loaded_models` returns only models whose state is "ready".
    #[test]
    fn loaded_models_filters_to_ready_only() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                state: "idle".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "c".into(),
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "d".into(),
                state: "loading".into(),
                ..Default::default()
            },
        ];

        let loaded = loaded_models(&models);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "a");
        assert_eq!(loaded[1].id, "c");
    }

    /// `loaded_models` returns an empty vec when all models are idle.
    #[test]
    fn loaded_models_returns_empty_when_none_ready() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                state: "idle".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                state: "failed".into(),
                ..Default::default()
            },
        ];

        let loaded = loaded_models(&models);
        assert!(loaded.is_empty());
    }

    /// `loaded_models` returns a clone of all models when all are ready.
    #[test]
    fn loaded_models_returns_all_when_all_ready() {
        let models = vec![
            ModelStatus {
                id: "x".into(),
                state: "ready".into(),
                ..Default::default()
            },
        ];

        let loaded = loaded_models(&models);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "x");
    }

    /// `loaded_models` returns an empty vec for an empty input slice.
    #[test]
    fn loaded_models_returns_empty_for_empty_input() {
        let models: Vec<ModelStatus> = vec![];
        let loaded = loaded_models(&models);
        assert!(loaded.is_empty());
    }
```

#### Step C: Filter the render list and fix empty-state UX

In the `Dashboard` component's reactive closure (locate the line `let models: Vec<ModelStatus> = buf.last().map(|h| h.models.clone()).unwrap_or_default();`):

Replace the existing models extraction and downstream logic with:

```rust
            let all_models: Vec<ModelStatus> = buf.last().map(|h| h.models.clone()).unwrap_or_default();
            let models = loaded_models(&all_models);

            view! { ... }
```

Then update the empty-state check (locate the `if models.is_empty()` block):

```rust
                        if all_models.is_empty() {
                            view! {
                                <div class="card card--centered">
                                    <p class="text-muted">"No models configured yet."</p>
                                </div>
                            }.into_any()
                        } else if models.is_empty() {
                            view! {
                                <div class="card card--centered">
                                    <p class="text-muted">"No models currently loaded."</p>
                                </div>
                            }.into_any()
                        } else {
                            // Sort by id (stable order, matching the backend)
                            let mut sorted = models;
                            sorted.sort_by(|a, b| a.id.cmp(&b.id));
                            // ... rest unchanged
                        }
```

Update the "X loaded" summary heading (locate the `{format!("{} loaded", loaded_model_count(&models))}` line):

```rust
                        <span class="text-muted">
                            {format!("{} loaded", models.len())}
                        </span>
```

(Since `models` is now the filtered list, `models.len()` equals `loaded_model_count(&models)` — this is correct because the heading sits above a list that only shows loaded models, so the count is self-evident.)

**Steps:**
- [ ] Write failing tests for `loaded_models()` in `crates/tama-web/src/pages/dashboard.rs` (the four tests above)
- [ ] Run `cargo test --package tama-web` — verify the four new tests fail (they reference an undefined function)
- [ ] Implement `loaded_models()` helper function in `crates/tama-web/src/pages/dashboard.rs`
- [ ] Run `cargo test --package tama-web` — verify all four tests pass
- [ ] Update the models extraction in `Dashboard` to use `all_models` + `loaded_models(&all_models)` (locate: `let models: Vec<ModelStatus> = buf.last()...`)
- [ ] Update the empty-state check to differentiate "no models configured" from "no models currently loaded"
- [ ] Update the "X loaded" heading to use `models.len()` (locate: `loaded_model_count(&models)`)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web` — fix any warnings
- [ ] Run `cargo check --package tama-web` to verify WASM compilation
- [ ] Run `cargo test --package tama-web` — verify all tests pass (including existing ones)
- [ ] Commit with message: "feat(dashboard): filter Active Models to show only loaded models, fix empty-state UX" or "feat(dashboard): filter Active Models to loaded-only with proper empty-state UX"

**Acceptance criteria:**
- [ ] The dashboard's "Active Models" section only renders model rows for models with `state == "ready"`
- [ ] Idle, loading, unloading, and failed models are no longer visible in the Active Models section
- [ ] When no models are configured at all, the message "No models configured yet." appears
- [ ] When models are configured but none are loaded, the message "No models currently loaded." appears
- [ ] The "X loaded" count correctly shows the number of ready models
- [ ] Four new unit tests for `loaded_models()` pass and cover: mixed states, none ready, all ready, empty input
- [ ] All existing unit tests in `dashboard.rs` still pass without modification
- [ ] `cargo fmt --all`, `cargo clippy --package tama-web`, and `cargo test --package tama-web` all succeed with no warnings
