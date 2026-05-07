# Model Editor Redesign — Implementation Plan

**Goal:** Redesign the model editor page to match the config editor's side-nav + main form area layout pattern, splitting the single scrolling form into 4 tabbed sections with vertical field layout and consolidated state management.

**Architecture:** The monolithic `ModelEditor` component (1795 lines) is restructured into a page shell with side-nav tabs (`Section` enum) and separate form section components (`GeneralForm`, `SamplingForm`, `QuantsVisionForm`, `ExtraArgsForm`). State consolidates from ~15 individual `RwSignal`s to a single `RwSignal<Option<ModelForm>>` plus UI signals. The layout switches from 2-column `form-grid` to vertical label-on-top stacks inside cards, matching the config editor's visual style. Save/Delete actions move to a sticky page header.

**Tech Stack:** Leptos 0.7 (WASM), Rust, existing CSS classes (`card`, `btn`, `form-input`, etc.)

---

### Task 1: Extract `target_value` helper to shared utility

**Context:**
The `target_value` function (which extracts a value from an input/select/textarea event) is currently defined in `config_editor.rs` and will be needed by the model editor's new section components. It should be in a shared location so both can use it. This also includes updating config_editor to import from the new location.

**Files:**
- Create: `crates/tama-web/src/utils.rs`
- Modify: `crates/tama-web/src/lib.rs` (add `mod utils`)
- Modify: `crates/tama-web/src/pages/config_editor.rs` (remove local `target_value`, import from `crate::utils`)

**What to implement:**
- Create `crates/tama-web/src/utils.rs` containing the `pub fn target_value(ev: &leptos::ev::Event) -> String` function, identical to the one currently in `config_editor.rs`. This function handles `<input>`, `<select>`, and `<textarea>` elements.
- In `crates/tama-web/src/lib.rs`, add `pub mod utils;` alongside the existing module declarations.
- In `config_editor.rs`, remove the local `fn target_value` function and add `use crate::utils::target_value;`.
- In `model_editor.rs`, replace all `event_target_value(&e)` calls with `target_value(&ev)`. Note: the model editor currently uses Leptos's `event_target_value` which only handles `<input>` elements. The new `target_value` from `crate::utils` also handles `<select>` and `<textarea>`, making it more robust.

**Important note:** `rw_signal_to_signal` (line 13), `format_bytes_opt` (line 78), and `short_sha` (line 97) are private helper functions in `model_editor.rs`. These remain as private functions in the same file and will be accessible to all section components since Leptos components in the same file share scope.

**Steps:**
- [ ] Create `crates/tama-web/src/utils.rs` with the `target_value` function
- [ ] Add `pub mod utils;` to `crates/tama-web/src/lib.rs`
- [ ] Remove `fn target_value` from `config_editor.rs` and add `use crate::utils::target_value;`
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix any import errors and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: extract target_value helper to shared utils module"

**Acceptance criteria:**
- [ ] `target_value` is available at `crate::utils::target_value`
- [ ] Config editor compiles and works identically
- [ ] No `target_value` duplicate remains in `config_editor.rs`
- [ ] All tama-web tests pass

---

### Task 2: Add Section enum, side-nav layout, consolidated state, and page header

**Context:**
This is the core structural change. The `ModelEditor` component currently uses ~15 individual `RwSignal`s and renders everything in a single scrolling form. This task adds the `Section` enum, converts state management to `RwSignal<Option<ModelForm>>`, builds the side-nav + main area layout, and moves Save/Delete buttons to the page header. All form content remains in the component body (inside match arms for each section) — extraction into separate components happens in Tasks 3-6. The layout switches from `form-grid` to vertical label-on-top stacks.

**Files:**
- Modify: `crates/tama-web/src/pages/model_editor.rs`

**What to implement:**

1. **Add `Section` enum** at the top of the file (after the existing types):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Sampling,
    QuantsVision,
    ExtraArgs,
}

impl Section {
    fn name(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Sampling => "Sampling",
            Section::QuantsVision => "Quants & Vision",
            Section::ExtraArgs => "Extra Args",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Section::General => "📋",
            Section::Sampling => "🎲",
            Section::QuantsVision => "📦",
            Section::ExtraArgs => "⚡",
        }
    }
}
```

2. **Consolidate state management**: Replace all individual form field signals (`form_id`, `form_backend`, `form_model`, `form_quant`, `form_args`, `form_enabled`, `form_context_length`, `form_port`, `api_name_field`, `form_gpu_layers`, `sampling_fields`, `quants`) with a single `RwSignal<Option<ModelForm>>` (the `ModelForm` struct already exists). Also keep `backends: RwSignal<Vec<String>>` for the backend dropdown.

   **Vision state migration:** The current `form_vision_enabled: RwSignal<bool>` and `selected_mmproj_for_config: RwSignal<String>` are consolidated into `ModelForm.mmproj: Option<String>`. Vision is "enabled" when `mmproj.is_some()`. The vision toggle checkbox reads `form.get().and_then(|f| f.mmproj.as_ref()).is_some()` and writes `form.update(|f| { if let Some(f) = f { f.mmproj = if checked { Some(selected_mmproj_for_config.get()) } else { None } })`. The mmproj dropdown reads `form.get().and_then(|f| f.mmproj.clone())` and writes `form.update(|f| { if let Some(f) = f { f.mmproj = Some(v); } })`. The available mmproj options are derived from `form.get().map(|f| f.quants.iter().filter(|(_, q)| q.kind == QuantKind::Mmproj).map(|(k, _)| k.clone()).collect::<Vec<_>>()).unwrap_or_default()`. The current `available_mmprojs_for_select` signal is eliminated — it's computed from the quants data.

   **Save action vision migration:** The current save logic `if form_vision_enabled.get() { Some(selected_mmproj_for_config.get()) } else { None }` becomes simply `form.get().and_then(|f| f.mmproj.clone())`. However, an empty mmproj string should be normalized to `None`: `mmproj = f.mmproj.filter(|s| !s.trim().is_empty())`.

   Keep these UI-only signals separate:
   - `current: RwSignal<Section>` — active tab
   - `loading: RwSignal<bool>`
   - `error: RwSignal<Option<String>>`
   - `save_status: RwSignal<Option<(bool, String)>>` — note: tuple type `(bool, String)`, where the boolean drives CSS class (`alert--success` vs `alert--error`) and icon (`✓` vs `✕`), matching the current `model_status` signal
   - `deleted: RwSignal<bool>`
   - `original_id: RwSignal<String>`
   - `pull_modal_open: RwSignal<bool>`
   - `refresh_busy: RwSignal<bool>`
   - `verify_busy: RwSignal<bool>`
   - `refresh_status: RwSignal<Option<(bool, String)>>`
   - `verify_status: RwSignal<Option<(bool, String)>>`
   - `repo_commit_sha: RwSignal<Option<String>>` — repo-level metadata, not part of ModelForm
   - `repo_pulled_at: RwSignal<Option<String>>` — repo-level metadata, not part of ModelForm

3. **Populate `ModelForm` on data load**: In the `Effect::new` that runs when `detail` loads, set the single `form` signal from `ModelDetail` data instead of individual signals. Map each `ModelDetail` field to the corresponding `ModelForm` field. Include the mmproj field and sampling map population.

4. **Update all save/delete/refresh/verify actions** to read from and write to the consolidated `form` signal. The `save_model` call remains the same but constructs the request from `form.get()`. **Note:** The current `model_status` signal is renamed to `save_status` but keeps the same `Option<(bool, String)>` type — the boolean drives styling and icon selection.

5. **Update `load_preset_action`** to use `form.update(|f| f.sampling...)` instead of the old `sampling_fields.update(|fields| ...)`. Each sampling field update becomes `form.update(|f| { if let Some(f) = f { f.sampling.entry("temperature").and_modify(|sf| { sf.enabled = true; sf.value = val; }).or_insert(SamplingField { enabled: true, value: val }); } })`. **This must be done in this task** — the old `sampling_fields` signal won't exist after consolidation.

6. **Update `delete_quant_action`** to use consolidated `form` signal. The current action updates 5 separate signals: `quants` (remove entry), `available_mmprojs_for_select` (remove if mmproj), `form_quant` (clear if matching), `selected_mmproj_for_config` / `form_vision_enabled` (clear if mmproj match). In the new model, this becomes:
   ```rust
   form.update(|f| {
       if let Some(f) = f {
           // Remove the quant entry
           f.quants.remove(&key);
           // Clear active quant reference if it matched
           if f.quant.as_deref() == Some(&key) { f.quant = None; }
           // Clear mmproj reference if it matched
           if f.mmproj.as_deref() == Some(&key) { f.mmproj = None; }
       }
   });
   ```
   The `model_status` update becomes `save_status.set(Some((true, "Quant deleted from disk.".into())))`.

7. **Convert all field bindings** from individual signals to `form.update(|f| ...)` pattern:
    - `prop:value=move || form.get().map(|f| f.backend.clone()).unwrap_or_default()`
    - `on:input=move |ev| { let v = target_value(&ev); form.update(|f| { if let Some(f) = f { f.backend = v; } }); }`
    - Replace all `event_target_value(&e)` calls with `target_value(&ev)` (the shared utility from Task 1)
    - Import `use crate::utils::target_value;`

8. **Convert sampling fields binding**: Instead of `sampling_fields.update(|fields| ...)`, use `form.update(|f| { if let Some(f) = f { f.sampling.entry("temperature").and_modify(|sf| { sf.value = v; sf.enabled = checked; }).or_insert(SamplingField { enabled: checked, value: v }); } })`

9. **Convert quants binding**: Instead of `quants.update(|rows| ...)`, use `form.update(|f| { if let Some(f) = f { f.quants ... } })` — adapt the `BTreeMap<String, QuantInfo>` operations. The `For` loop iterates over `form.get().map(|f| f.quants.clone().into_iter().collect::<Vec<_>>()).unwrap_or_default()`.

10. **Restructure the view** into:
    - Page header with title, Save/Delete buttons, and `save_status` (using `(bool, String)` tuple for status styling: success → `alert--success` + `✓`, failure → `alert--error` + `✕`)
    - Flex layout: side-nav (220px card) + main form area (flex:1)
    - Side-nav: 4 buttons (General, Sampling, Quants & Vision, Extra Args) with `class:btn-primary`/`class:btn-secondary` toggling like config editor
    - Main area: match on `current.get()` to render the appropriate section's form content
    - Each section renders inside a `<div class="card">` with vertical label-on-top fields using `display:flex;flex-direction:column;gap:1rem;margin-top:1rem;`
    - **Remove the `<div class="form-card">` wrapper** — replaced by the flex layout with side-nav + card per section
    - **Remove the `<form on:submit=...>` wrapper** — Save is now a button click in the header, not a form submit. Note: Enter key will no longer submit the form (prevents accidental submission, which is desirable in an editor)
    - **Remove `<h3 class="form-section-title">` headings** — replaced by `<h2>` titles inside section cards
    - **Remove `<hr class="section-divider">` between sections** — sections are now separate cards, no divider needed

11. **Keep the ModelEditor component as a single function for now** — don't extract section components yet. All form content for each section lives in the match arms of the main component. Private helper functions (`rw_signal_to_signal`, `format_bytes_opt`, `short_sha`) remain in the same file and are accessible to all section components since Leptos components in the same file share scope.

12. **Keep the Modal + PullQuantWizard** wrapping as-is outside the flex layout. It stays at the bottom of the view.

**Important notes:**
- The `ModelForm` struct's `quants` field is `BTreeMap<String, QuantInfo>`. The current code uses `Vec<(String, QuantInfo)>` for the `For` loop. Derive the Vec on-the-fly from `form.get().quants.clone().into_iter().collect::<Vec<_>>()`.
- The `save_status` signal keeps `Option<(bool, String)>` type (not `Option<String>`) to preserve success/failure distinction for styling.

**Steps:**
- [ ] Add `Section` enum with `name()` and `icon()` methods to `model_editor.rs`
- [ ] Add `use crate::utils::target_value;` import
- [ ] Replace all `event_target_value(&e)` calls with `target_value(&ev)` throughout the file
- [ ] Replace individual form signals with `let form: RwSignal<Option<ModelForm>> = RwSignal::new(None);`
- [ ] Consolidate `form_vision_enabled` and `selected_mmproj_for_config` into `form.mmproj: Option<String>`
- [ ] Eliminate `available_mmprojs_for_select` — derive mmproj options from `form.quants`
- [ ] Keep `backends`, `original_id`, `pull_modal_open`, UI status signals (including `repo_commit_sha`, `repo_pulled_at`) as individual signals
- [ ] Rename `model_status` to `save_status` (keeping `Option<(bool, String)>` type)
- [ ] Update the `Effect::new` data population block to set the `form` signal from `ModelDetail`
- [ ] Update `save_action` to read from `form.get()` and handle mmproj normalization
- [ ] Update `load_preset_action` to use `form.update(|f| f.sampling...)`
- [ ] Update `delete_quant_action` to use `form.update(|f| f.quants...)` with mmproj cleanup
- [ ] Update `delete_action`, `refresh_action`, `verify_action` to use consolidated signal
- [ ] Restructure the view: remove `<form>` and `<div class="form-card">` wrappers, add page header with Save/Delete buttons and `save_status`, add side-nav with section buttons, add main form area with match on `current.get()`
- [ ] Remove `<h3 class="form-section-title">` headings and `<hr class="section-divider">`
- [ ] Convert all form field bindings from individual signals to `form.update()` pattern
- [ ] Convert sampling fields to use `form.update(|f| f.sampling...)`
- [ ] Convert quants rendering to derive Vec from `form.get().quants`
- [ ] Switch form layout from `form-grid` to vertical label-on-top in `<div class="card">` sections
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix errors and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
  - Did it pass? If not, fix warnings and re-run.
- [ ] Commit with message: "refactor: redesign model editor with side-nav tabs and consolidated state"

**Acceptance criteria:**
- [ ] Model editor shows 4-section side-nav (General, Sampling, Quants & Vision, Extra Args)
- [ ] Clicking a tab switches the visible form section
- [ ] All existing functionality works: save, delete, rename, refresh, verify, pull quant wizard
- [ ] Form fields use vertical label-on-top layout (not form-grid)
- [ ] Save/Delete buttons appear in the page header, always visible
- [ ] State managed by single `RwSignal<Option<ModelForm>>`
- [ ] All clippy warnings resolved, all tests pass

---

### Task 3: Extract `GeneralForm` component

**Context:**
The General section's form content is currently inline in the ModelEditor match arm. Extract it into a separate `GeneralForm` component that receives `form: RwSignal<Option<ModelForm>>` and `backends: RwSignal<Vec<String>>` as props. This makes the code match the config editor's pattern where each section is a separate component.

**Files:**
- Modify: `crates/tama-web/src/pages/model_editor.rs`

**What to implement:**

Create a private component:
```rust
#[component]
fn GeneralForm(form: RwSignal<Option<ModelForm>>, backends: RwSignal<Vec<String>>) -> impl IntoView {
    view! {
        <div class="card">
            <h2>"General"</h2>
            <p class="text-muted">"Basic model configuration fields."</p>
            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                // ID field
                // Backend dropdown
                // API Name
                // Model (HF repo)
                // Quant dropdown
                // GPU Layers
                // Context length
                // Port override
                // Enabled checkbox
                // Load Preset dropdown
            </div>
        </div>
    }
}
```

Each field uses the vertical layout with `form.update(|f| ...)` bindings. The `backends` signal is passed as a prop for the backend dropdown.

In the `ModelEditor` match arm, replace the inline General content with:
```rust
Section::General => view! { <GeneralForm form=form backends=backends /> }.into_any(),
```

**Steps:**
- [ ] Create `GeneralForm` component function with proper props
- [ ] Move all General section field markup from the match arm into `GeneralForm`
- [ ] Replace inline content in `ModelEditor` match arm with `<GeneralForm form=form backends=backends />`
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix errors and re-run.
- [ ] Run `cargo test --package tama-web`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: extract GeneralForm component from model editor"

**Acceptance criteria:**
- [ ] General section renders correctly in its own component
- [ ] All General fields (ID, Backend, API Name, Model, Quant, etc.) still work
- [ ] Backend dropdown still populates from `backends` signal
- [ ] Preset dropdown still works

---

### Task 4: Extract `SamplingForm` component

**Context:**
The Sampling section has 7 per-parameter checkbox+input pairs with a preset dropdown. Extract it into a `SamplingForm` component that receives `form: RwSignal<Option<ModelForm>>` and the templates `LocalResource` for preset loading.

**Files:**
- Modify: `crates/tama-web/src/pages/model_editor.rs`

**What to implement:**

Create a private component:
```rust
#[component]
fn SamplingForm(
    form: RwSignal<Option<ModelForm>>,
    templates: LocalResource<..., Option<...>>,
    load_preset_action: Action<String, (), LocalStorage>,
) -> impl IntoView { ... }
```

The sampling fields (temperature, top_k, top_p, min_p, presence_penalty, frequency_penalty, repeat_penalty) all use the checkbox+input pattern where the checkbox enables/disables the field. Each one reads from `form.get().and_then(|f| f.sampling.get("temperature")).map(|sf| ...)`. Each input writes via `form.update(|f| f.sampling.entry("temperature").and_modify(|sf| sf.value = v).or_insert(SamplingField { enabled: true, value: v }))`.

The preset dropdown dispatches `load_preset_action` which now updates the `form` signal's `sampling` field instead of a separate `sampling_fields` signal.

**Steps:**
- [ ] Create `SamplingForm` component with `form`, `templates`, and `load_preset_action` props
- [ ] Move all Sampling section markup from the match arm into `SamplingForm`
- [ ] Update `load_preset_action` to use `form.update(|f| f.sampling...)` instead of `sampling_fields.update(|fields| ...)`
- [ ] Replace inline content in `ModelEditor` match arm with `<SamplingForm form=form templates=templates load_preset_action=load_preset_action />`
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix errors and re-run.
- [ ] Run `cargo test --package tama-web`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: extract SamplingForm component from model editor"

**Acceptance criteria:**
- [ ] Sampling section renders correctly in its own component
- [ ] All 7 sampling parameters work with checkbox toggle and value input
- [ ] Preset dropdown still loads and populates sampling fields
- [ ] Enable/disable checkboxes correctly toggle field state

---

### Task 5: Extract `QuantsVisionForm` component

**Context:**
The Quantizations & Vision section is the most complex — it includes the quants table with inline editing, Pull Quant button/modal, Refresh/Verify action buttons, mmproj toggle and dropdown, and repo-level metadata display. Extract it into `QuantsVisionForm` which receives the `form` signal plus several UI signals and actions.

**Files:**
- Modify: `crates/tama-web/src/pages/model_editor.rs`

**What to implement:**

Create a private component:
```rust
#[component]
fn QuantsVisionForm(
    form: RwSignal<Option<ModelForm>>,
    original_id: RwSignal<String>,
    is_new: impl Fn() -> bool + Clone + 'static,
    refresh_action: Action<(), (), LocalStorage>,
    verify_action: Action<(), (), LocalStorage>,
    refresh_busy: RwSignal<bool>,
    verify_busy: RwSignal<bool>,
    refresh_status: RwSignal<Option<(bool, String)>>,
    verify_status: RwSignal<Option<(bool, String)>>,
    repo_commit_sha: RwSignal<Option<String>>,
    repo_pulled_at: RwSignal<Option<String>>,
    delete_quant_action: Action<(String, String), (), LocalStorage>,
    pull_modal_open: RwSignal<bool>,
) -> impl IntoView { ... }
```

**Note on prop count:** This component has ~12 props, which is more than ideal. This is justified because QuantsVisionForm has significant specialized behavior (quant CRUD, refresh/verify actions, mmproj logic, pull wizard integration). These props cannot be consolidated into the `form` signal since they are UI actions or async state. If desired later, the action and status signals could be grouped into a struct (e.g., `QuantsActions`), but that's a separate refactor for clarity, not functionality.

This component renders:
1. Quants meta bar (commit SHA, pulled date, refresh/verify buttons)
2. Quants table with inline editing, delete buttons, status indicators
3. Pull Quant button (opens modal)
4. Vision projector toggle + mmproj dropdown
5. Refresh/verify status alerts

The `For` loop for quants iterates over `form.get().quants.clone().into_iter().collect::<Vec<_>>()`.

The mmproj toggle reads from `form.get().and_then(|f| f.mmproj.as_ref())` — if mmproj is Some, vision is enabled. The available mmproj options are derived from `form.get().quants.iter().filter(|(_, q)| q.kind == QuantKind::Mmproj)`.

**Steps:**
- [ ] Create `QuantsVisionForm` component with all required props
- [ ] Move quants meta bar, quants table, pull quant button, refresh/verify actions
- [ ] Move vision projector toggle and mmproj dropdown
- [ ] Move refresh/verify status alert markup
- [ ] Adapt quants rendering to work from `form` signal's `quants` BTreeMap
- [ ] Replace inline content in `ModelEditor` match arm with `<QuantsVisionForm ... />`
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix errors and re-run.
- [ ] Run `cargo test --package tama-web`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Commit with message: "refactor: extract QuantsVisionForm component from model editor"

**Acceptance criteria:**
- [ ] Quant table renders and allows inline editing
- [ ] Pull Quant button opens the modal wizard
- [ ] Refresh and Verify buttons work
- [ ] Delete quant button works (with confirmation)
- [ ] Vision projector toggle shows/hides mmproj dropdown
- [ ] mmproj dropdown lists available mmproj files from quants

---

### Task 6: Extract `ExtraArgsForm` component and final cleanup

**Context:**
The Extra Args section is the simplest — just a textarea for extra args with a hint. Extract it, then clean up any remaining inline code, remove unused `form-grid` references from model_editor, and verify the entire page works end-to-end.

**Files:**
- Modify: `crates/tama-web/src/pages/model_editor.rs`
- Modify: `crates/tama-web/style.css` (optional: if any model-editor-specific styles need updating)

**What to implement:**

1. Create `ExtraArgsForm` component:
```rust
#[component]
fn ExtraArgsForm(form: RwSignal<Option<ModelForm>>) -> impl IntoView {
    view! {
        <div class="card">
            <h2>"Extra Arguments"</h2>
            <p class="text-muted">"Additional command-line flags, one per line."</p>
            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Arguments"</label>
                    <textarea
                        class="form-textarea"
                        rows="6"
                        placeholder="One flag per line, e.g.:\n-fa 1\n-b 4096\n--mlock"
                        prop:value=move || form.get().map(|f| f.args.join("\n")).unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            form.update(|f| {
                                if let Some(f) = f {
                                    f.args = v.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                                }
                            });
                        }
                    />
                    <span class="form-hint">"One flag per line, e.g. -fa 1, --mlock, or -b 4096"</span>
                </div>
            </div>
        </div>
    }
}
```

2. Replace the inline ExtraArgs match arm content with `<ExtraArgsForm form=form />`.

3. **Cleanup:**
   - Ensure the `ModelEditor` component is clean — no leftover inline form sections
   - Verify `form-grid` class is no longer used in model_editor (it may still be used elsewhere, that's fine)
   - Ensure the `Modal` and `PullQuantWizard` are still properly wired (they live outside the section routing, at the bottom of the main view)
   - Verify the deleted-state redirect still works
   - Verify the rename logic still works (old_id vs new_id comparison in save action)

4. **Visual verification** (manual):
   - Check that the page renders correctly with dark theme
   - Side-nav buttons highlight correctly for active section
   - Form fields display in vertical stack with proper spacing
   - All field values persist when switching tabs (state doesn't reset)
   - The `hr.section-divider` between sections is no longer needed (each section is a separate card)

**Steps:**
- [ ] Create `ExtraArgsForm` component
- [ ] Replace inline content in `ModelEditor` match arm
- [ ] Remove any leftover `form-section-title` headers (replaced by card `<h2>` titles)
- [ ] Remove `hr.section-divider` that separated sections (no longer needed)
- [ ] Verify Modal + PullQuantWizard wiring still works
- [ ] Verify rename logic (old_id vs new_id) still works
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix errors and re-run.
- [ ] Run `cargo test --package tama-web`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: "refactor: extract ExtraArgsForm and finalize model editor redesign"

**Acceptance criteria:**
- [ ] Extra Args section renders correctly in its own component
- [ ] No inline form content remaining in `ModelEditor` component (only section routing + shared state/actions)
- [ ] `ModelEditor` component is limited to: state setup, actions, side-nav, section match, Modal
- [ ] All 4 sections work correctly when navigated via side-nav
- [ ] Form state persists across tab switches
- [ ] Save, Delete, Rename, Refresh, Verify, Pull Quant all work
- [ ] Deleted state redirect works
- [ ] No clippy warnings, all tests pass