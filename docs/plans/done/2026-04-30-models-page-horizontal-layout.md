# Models Page Horizontal Layout Plan

**Goal:** Replace the models page's vertical card grid layout with the same horizontal row layout used by the dashboard's Active Models section.

**Architecture:** Single flat list of model rows (no loaded/unloaded sections, no sorting). Each row displays: Name, Quant, Backend, Enabled badge, State badge, Actions (Load/Unload, Edit). Reuses existing CSS classes (`models-list`, `model-row`, `model-row__name`, `model-row__meta`, `model-row__backend`, `model-row__actions`, `badge`).

**Divergences from dashboard row:**
- Two badges (Enabled + State) instead of one (State only) — both placed inside `model-row__actions`
- No context-length meta field (`ModelEntry` doesn't have `context_length`)
- No `model` (repo name) field — display name suffices for identity in horizontal layout
- Models appear in backend response order (no sorting)

**Tech Stack:** Rust, Leptos (WASM), CSS

---

## Task 1: Convert models page to horizontal row layout

**Context:**
The models page (`pages/models.rs`) currently renders models in two sections ("Loaded Models" and "Unloaded Models") using vertical `model-card` components in a `models-grid` CSS grid. The dashboard's Active Models section uses horizontal `model-row` items in a `models-list` flex column. This task replaces the card layout with the row layout, removing the partition logic and duplicating view code.

The reviewer noted these concerns that are addressed in this task:
- `ModelEntry.id` is `i64` — use `.to_string()` for action dispatch and edit URLs (existing pattern)
- No sorting — models appear in backend response order (user's explicit choice)
- Badges use standard `badge` classes (`badge badge-success`, etc.) — the `.model-row .badge` CSS rule handles styling within rows
- Closure capture: pre-clone `id_load`, `id_unload`, `id_edit` before each `view!` macro (existing pattern)

**Files:**
- Modify: `crates/tama-web/src/pages/models.rs`

**What to implement:**

1. **Remove the `partition_models_by_loaded` function** (lines ~28-34):
   ```rust
   fn partition_models_by_loaded(models: Vec<ModelEntry>) -> (Vec<ModelEntry>, Vec<ModelEntry>) {
       // ... entire function
   }
   ```

2. **Replace the `Some(data) =>` branch** in the `Suspense` body (currently rendering two `model-section` + `models-grid` + `model-card` blocks) with a single flat list:

   ```rust
   Some(data) => {
       view! {
           <div class="models-list">
               {data.models.into_iter().map(|m| {
                   let id_load = m.id.to_string();
                   let id_unload = m.id.to_string();
                   let id_edit = m.id.to_string();
                   let enabled_class = if m.enabled { "badge badge-success" } else { "badge badge-warning" };
                   let (state_label, state_class) = model_state_badge(&m.state);
                   view! {
                       <div class="model-row card">
                           <span class="model-row__name">{model_display_name(&m)}</span>
                           <span class="model-row__meta">{m.quant.as_deref().unwrap_or("\u{2014}")}</span>
                           <span class="model-row__backend text-mono">{m.backend}</span>
                           <div class="model-row__actions">
                               <span class=enabled_class>
                                   {if m.enabled { "Enabled" } else { "Disabled" }}
                               </span>
                               <span class={format!("badge {}", state_class)}>{state_label}</span>
                               {if m.loaded {
                                   view! {
                                       <button
                                           class="btn btn-danger btn-sm"
                                           on:click=move |_| {
                                               unload_action.dispatch(id_unload.clone());
                                               refresh.update(|n| *n += 1);
                                           }
                                       >
                                           "Unload"
                                       </button>
                                   }.into_any()
                               } else {
                                   view! {
                                       <button
                                           class="btn btn-success btn-sm"
                                           on:click=move |_| {
                                               load_action.dispatch(id_load.clone());
                                               refresh.update(|n| *n += 1);
                                           }
                                       >
                                           "Load"
                                       </button>
                                   }.into_any()
                               }}
                               <A href=format!("/models/{}/edit", id_edit)>
                                   <button class="btn btn-secondary btn-sm">"Edit"</button>
                               </A>
                           </div>
                       </div>
                   }
               }).collect::<Vec<_>>()}
           </div>
       }.into_any()
   }
   ```

3. **Keep unchanged:**
   - Page header with "Check all for updates" and "Pull Model" buttons
   - Status alert rendering (`check_all_status`)
   - Empty state ("No models configured yet." + "Pull a Model" button)
   - Error state ("Failed to load models.")
   - `LocalResource::new` fetching logic
   - `load_action`, `unload_action`, `check_all_action` definitions
   - Pull modal with `PullQuantWizard`
   - `rw_signal_to_signal` helper (used by modal)
   - `model_display_name` and `model_state_badge` helper functions

4. **Remove tests** for `partition_models_by_loaded` (5 tests in `#[cfg(test)] mod tests`):
   - `test_all_loaded_returns_n_zero`
   - `test_all_unloaded_returns_zero_n`
   - `test_mixed_returns_correct_split`
   - `test_empty_returns_zero_zero`
   - `test_sorts_both_partitions_by_id`

   If the tests module becomes empty after removal, remove the entire `#[cfg(test)] mod tests` block.

**Steps:**
- [ ] Remove `partition_models_by_loaded` function from `crates/tama-web/src/pages/models.rs`
- [ ] In the `Some(data) =>` branch, remove the line `let (loaded, unloaded) = partition_models_by_loaded(data.models);` and the two conditional `model-section` blocks that render loaded/unloaded sections
- [ ] Replace with the new `models-list` + `model-row` view block (see code snippet above)
  - Badges (Enabled + State) go INSIDE `model-row__actions` div, matching the dashboard pattern
  - Uses standard `badge` classes — the `.model-row .badge` CSS descendant selector handles styling
- [ ] Remove the 5 `partition_models_by_loaded` tests (and the entire `#[cfg(test)] mod tests` block if it becomes empty)
- [ ] Run `cargo check --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat(tama-web): convert models page to horizontal row layout"

**Notes:**
- Dead CSS (`.model-section`, `.models-grid`, `.model-card*`) is left in place — cleanup can be a follow-up
- `rw_signal_to_signal` helper is kept (used by the pull modal)

**Acceptance criteria:**
- [ ] Models page renders all models in a single flat horizontal list (no sections)
- [ ] Each row shows: Name, Quant, Backend, Enabled badge, State badge, Load/Unload button, Edit button
- [ ] No `partition_models_by_loaded` function remains
- [ ] No `models-grid` or `model-card` classes used on the models page
- [ ] `cargo check`, `cargo fmt`, `cargo clippy`, `cargo build` all pass for `tama-web`
- [ ] Header buttons ("Check all for updates", "Pull Model") still work
- [ ] Pull modal still opens and functions correctly
