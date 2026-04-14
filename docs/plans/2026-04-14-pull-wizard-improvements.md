# [Pull Wizard Improvements] Plan

**Goal:** Consolidate quant and vision selection into a single page and implement a smart dropdown for KV cache (context length) sizes.
**Architecture:** UI changes within the `PullQuantWizard` Leptos component.
**Tech Stack:** Rust, Leptos, WASM.

---

### Task 1: Update Wizard State and Step Indicators

**Context:**
The wizard currently has separate steps for quants and vision. We need to remove the `Vision` step from the state machine and update the visual progress indicator to reflect the new flow: Repo -> Loading -> Select -> Context -> Downloading -> Done.

**Files:**
- Modify: `crates/koji-web/src/components/pull_quant_wizard.rs`

**What to implement:**
1. Remove `Vision` variant from `WizardStep` enum.
2. Verify that `step_class` order array (around line 99) does NOT contain `WizardStep::Vision` — it should already be excluded.
3. Update the `view!` block for the wizard step indicator (around lines 340-369) to remove the "4. Vision" step and re-index the subsequent steps as follows:
    - Replace "5. Context" with "4. Context"
    - Replace "6. Download" with "5. Download"
    - Replace "7. Done" with "6. Done"

**Steps:**
- [ ] Remove `Vision` from `WizardStep` enum.
- [ ] Update the step indicator HTML in the `view!` block with the new numbering.
- [ ] Run `cargo build`
- [ ] Run `cargo fmt --all`
- [ ] Verify manually in browser that the step indicator shows 6 steps (Repo, Loading, Select, Context, Download, Done).
- [ ] Commit with message: "feat(web): remove Vision step from PullQuantWizard state machine"

**Acceptance criteria:**
- [ ] `WizardStep::Vision` is gone.
- [ ] The UI step indicator shows 6 steps with correct labels.
- [ ] Code compiles and formatting is correct.

---

### Task 2: Consolidate Selection Page (Quants + Vision)

**Context:**
Currently, users must select a quant before they can select a vision projector. We want them on one page so users can select either or both.

**Files:**
- Modify: `crates/koji-web/src/components/pull_quant_wizard.rs`

**What to implement:**
1. In the `WizardStep::SelectQuants` view:
    - Add a "Vision Projectors" section below the quants table.
    - Instead of a `select` dropdown, use a list of checkboxes for `available_mmprojs`, matching the style of the quants table.
    - Each checkbox should update `selected_mmproj_filenames`.
2. Update the "Next" button `prop:disabled` logic:
    - The button should be disabled ONLY if `selected_filenames` is empty AND `selected_mmproj_filenames` is empty.
3. Update the "Next" button `on:click` handler:
    - Transition directly to `WizardStep::SetContext`.
4. Remove the `WizardStep::Vision` view block entirely.
5. Note: The existing "Select All" / "Deselect All" buttons should only toggle model quants (`selected_filenames`), NOT vision projectors.

**Steps:**
- [ ] Implement the Vision Projectors checkbox list in `WizardStep::SelectQuants` view.
- [ ] Update "Next" button `prop:disabled` logic to allow progression if either quants or mmprojs are selected.
- [ ] Update "Next" button `on:click` to transition to `WizardStep::SetContext`.
- [ ] Remove the `WizardStep::Vision` match arm.
- [ ] Run `cargo build`
- [ ] Run `cargo fmt --all`
- [ ] Verify manually in browser:
    - [ ] Vision projectors are listed with checkboxes.
    - [ ] "Next" is enabled when only a vision projector is selected.
    - [ ] "Select All" does not affect vision projectors.
- [ ] Commit with message: "feat(web): consolidate quant and vision selection into one page"

**Acceptance criteria:**
- [ ] Vision projectors are selectable via checkboxes on the same page as quants.
- [ ] User can proceed to the next step by selecting only a vision projector.
- [ ] The `Vision` step is no longer reachable.

---

### Task 3: Implement Smart KV Cache Dropdown

**Context:**
Users currently type in the context length. We want a dropdown with linear increments and a "Custom" option.

**Files:**
- Modify: `crates/koji-web/src/components/pull_quant_wizard.rs`

**What to implement:**
1. In the `WizardStep::SetContext` view:
    - Replace the `input type="number"` for context length with a combination of a `select` dropdown and a conditional `input`.
    - The `select` must contain exactly these hardcoded values as options: `2048, 4096, 8192, 16384, 32768, 49152, 65536, 81920, 98304, 131072, 147456, 163840, 196608, 262144, 524288, 1048576`.
    - Add a `"Custom..."` option to the end of the `select` list.
    - If `"Custom..."` is selected, show the numeric `input` to allow free-form entry.
    - Selecting any value from the dropdown should immediately update `context_lengths` for that filename.
2. Update the "Back" button logic:
    - It should now always go back to `WizardStep::SelectQuants`.

**Steps:**
- [ ] Implement the `select` dropdown with the specified hardcoded values and "Custom..." option.
- [ ] Implement the conditional numeric `input` for the "Custom..." case.
- [ ] Wire up the `on:change` handlers to update `context_lengths`.
- [ ] Update the "Back" button to point to `WizardStep::SelectQuants`.
- [ ] Run `cargo build`
- [ ] Run `cargo fmt --all`
- [ ] Verify manually in browser:
    - [ ] Dropdown contains all specified values.
    - [ ] Selecting a value updates the context length.
    - [ ] Selecting "Custom..." shows the numeric input.
    - [ ] "Back" button returns to the selection page.
- [ ] Commit with message: "feat(web): add smart dropdown for context length in pull wizard"

**Acceptance criteria:**
- [ ] Users can select a context length from the predefined list.
- [ ] Users can enter a custom value via the "Custom..." option.
- [ ] The "Back" button correctly returns to the selection page.
