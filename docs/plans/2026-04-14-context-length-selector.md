# [Context Length Selector] Plan

**Goal:** Implement a reusable `ContextLengthSelector` component to standardize context length input across the Model Editor and Pull Quant Wizard, eliminating "cursor-jump" bugs and providing a consistent dropdown + custom input UI.
**Architecture:** A controlled Leptos component that manages internal "Custom" toggle state while delegating value updates to parents via callbacks. It uses an imperative DOM update pattern for the numeric input to avoid reactive overwrite issues.
**Tech Stack:** Rust, Leptos (WASM).

---

### Task 1: Setup Shared Constants

**Context:**
Standard context length presets are used in multiple places. Moving them to a central constants module avoids duplication and prevents circular dependencies between the `ModelEditor` and `PullQuantWizard`.

**Files:**
- Create: `crates/koji-web/src/constants.rs`
- Modify: `crates/koji-web/src/lib.rs`
- Modify: `crates/koji-web/src/components/pull_wizard/mod.rs`

**What to implement:**
1. Create `constants.rs` as a top-level module and export `pub const CONTEXT_VALUES: &[u32]`.
2. Register `pub mod constants;` in `lib.rs`.
3. Remove the hardcoded `CONTEXT_VALUES` array from `pull_wizard/mod.rs` and import it from `crate::constants`.

**Steps:**
- [ ] Create `crates/koji-web/src/constants.rs` with the 16 predefined context values.
- [ ] Add `pub mod constants;` to `crates/koji-web/src/lib.rs`.
- [ ] Replace local `CONTEXT_VALUES` in `crates/koji-web/src/components/pull_wizard/mod.rs` with `use crate::constants::CONTEXT_VALUES;`.
- [ ] Run `cargo build` to verify no compilation errors.
- [ ] Commit with message: "refactor(web): extract context length presets to shared constants"

**Acceptance criteria:**
- [ ] `CONTEXT_VALUES` is accessible from any module in `koji-web`.
- [ ] `pull_wizard` still functions correctly using the imported constants.

---

### Task 2: Implement `ContextLengthSelector` Component

**Context:**
The current numeric inputs in Leptos suffer from the "cursor-jump" bug when `prop:value` is updated reactively. This component solves this by using an imperative `set_value` call via a `StoredValue` reference to the DOM element, combined with a dropdown for common presets.

**Files:**
- Create: `crates/koji-web/src/components/context_length_selector.rs`
- Modify: `crates/koji-web/src/components/mod.rs`

**What to implement:**
1. Implement `ContextLengthSelector` with props: `value: Signal<Option<u32>>`, `on_change: Callback<Option<u32>>`, `reset_key: Signal<String>`, and `class: Option<String>`.
2. Internal state: `is_custom: RwSignal<bool>`.
3. Logic:
    - Dropdown shows `CONTEXT_VALUES` and `"Custom..."`.
    - Selecting a preset sets `is_custom = false` and calls `on_change`.
    - Selecting `"Custom..."` sets `is_custom = true`.
    - Numeric input is displayed only when `is_custom` is true.
    - **Crucial:** Use `display: none/block` for visibility to ensure the DOM element remains stable.
    - Use an `Effect` to imperatively update the numeric input's value from `value` signal only when `is_custom` is true and the value actually changes, using `StoredValue` to hold the `HtmlInputElement` reference.
    - **Crucial:** The `on:mount` closure must have an explicit type: `move |el: web_sys::Element|`.
    - Use an `Effect` to reset `is_custom` when `reset_key` changes.
    - Use an `Effect` to set `is_custom = false` if the external `value` changes to one of the presets.

**Steps:**
- [ ] Implement the component in `crates/koji-web/src/components/context_length_selector.rs`.
- [ ] Register the module in `crates/koji-web/src/components/mod.rs`.
- [ ] Run `cargo build`.
- [ ] Commit with message: "feat(web): implement ContextLengthSelector component"

**Acceptance criteria:**
- [ ] Component compiles without errors.
- [ ] Dropdown correctly toggles the visibility of the numeric input.
- [ ] Numeric input does not lose focus or jump cursor when typing.

---

### Task 3: Integrate in Model Editor - General Form

**Context:**
The global context length for a model needs the dropdown UI. Because closures do not auto-convert to `Signal` in Leptos 0.7, `Signal::derive` must be used.

**Files:**
- Modify: `crates/koji-web/src/pages/model_editor/general_form.rs`

**What to implement:**
1. Replace the `field-ctx` `<input type="number">` with `<ContextLengthSelector />`.
2. Bind `value` using `Signal::derive(move || form.get().and_then(|f| f.context_length))`.
3. Bind `on_change` to update `form.context_length` in the `RwSignal<Option<ModelForm>>`.
4. Pass the model ID as the `reset_key` using `Signal::derive(move || form.get().map(|f| f.id.clone()).unwrap_or_default())`.

**Steps:**
- [ ] Replace the numeric input in `general_form.rs`.
- [ ] Wire up the signals using `Signal::derive`.
- [ ] Run `cargo build`.
- [ ] Commit with message: "feat(web): use ContextLengthSelector in ModelEditor general form"

**Acceptance criteria:**
- [ ] Dropdown is visible in the General form.
- [ ] Selecting a preset updates the model's context length.
- [ ] Switching models resets the selector to the new model's value.

---

### Task 4: Integrate in Model Editor - Quants Form

**Context:**
Each quant in a model can have its own context length override. These are rendered in a table.

**Files:**
- Modify: `crates/koji-web/src/pages/model_editor/quants_vision_form.rs`

**What to implement:**
1. Replace the numeric input in the quants table with `<ContextLengthSelector />`.
2. Use `Arc<String>` to capture the quant name for the `on_change` callback.
3. Bind `value` using `Signal::derive(move || { ... })` looking up the quant's `context_length` in the `form` signal.
4. Pass `original_id` as the `reset_key` using `Signal::derive(move || original_id.get())`.
5. Apply `class="input-narrow"`.

**Steps:**
- [ ] Replace the numeric input in the `quants_vision_form.rs` table.
- [ ] Implement the closure captures for updating the specific quant.
- [ ] Run `cargo build`.
- [ ] Commit with message: "feat(web): use ContextLengthSelector in ModelEditor quants form"

**Acceptance criteria:**
- [ ] Each quant row has a functioning context length selector.
- [ ] Updating a quant's length is reflected in the form state.

---

### Task 5: Refactor Pull Quant Wizard

**Context:**
The wizard's `ContextFileDropdown` currently duplicates the dropdown logic. This should be replaced by the shared component.

**Files:**
- Modify: `crates/koji-web/src/components/pull_wizard/components/context_step.rs`

**What to implement:**
1. Remove the `is_custom` signal and the manual `<select>`/`<input>` logic from `ContextFileDropdown`.
2. Use `<ContextLengthSelector />` inside `ContextFileDropdown`.
3. Bind `value` using `Signal::derive(move || context_lengths.get().get(&filename).map(Some))`.
4. Bind `on_change` using `Callback::new(move |v: Option<u32>| { ... })` with an explicit type annotation.
5. Ensure the `is_custom` signal is removed from the `ContextStep` loop to avoid unused variable warnings.

**Steps:**
- [ ] Refactor `ContextFileDropdown` to use the new component.
- [ ] Remove the unused `is_custom` signal from the `ContextStep` loop.
- [ ] Run `cargo build`.
- [ ] Commit with message: "refactor(web): use ContextLengthSelector in Pull Quant Wizard"

**Acceptance criteria:**
- [ ] The wizard's context step maintains its existing behavior using the shared component.
- [ ] No compiler warnings for unused variables.

---

### Task 6: Verification and Tests

**Context:**
Verify the implementation and add basic tests to ensure the logic remains correct.

**Steps:**
- [ ] Create a test module for `ContextLengthSelector` (if possible in WASM) or verify via manual tests:
    - Verify presets $\rightarrow$ `is_custom = false`.
    - Verify "Custom..." $\rightarrow$ `is_custom = true`.
    - Verify `reset_key` change resets state.
- [ ] Open Model Editor $\rightarrow$ General Form: Verify presets, Custom input, and model switching.
- [ ] Open Model Editor $\rightarrow$ Quants Form: Verify independent row edits.
- [ ] Open Pull Quant Wizard: Verify the context step functions correctly.
- [ ] Run `cargo fmt --all` and `cargo build`.
- [ ] Commit with message: "test(web): verify context length selector integration"

**Acceptance criteria:**
- [ ] No "cursor-jump" behavior in any numeric input.
- [ ] All three UI locations correctly update their respective state.
- [ ] Code is formatted and builds successfully.
