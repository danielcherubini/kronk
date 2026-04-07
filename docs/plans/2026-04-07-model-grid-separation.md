# Model Grid Separation Plan (Final)

**Goal:** Separate the model grid into "Loaded Models" and "Unloaded Models" sections with visual headers between them.

**Architecture:** Split the existing single grid into two filtered sections based on the `loaded` field. Each section gets its own `<h2>` header with the section title. No backend changes needed - the `loaded` field already exists in the data model.

**Tech Stack:** Rust/Leptos frontend, CSS styling

---

## Task 0: Add failing unit tests for partition helper

**Context:**
The plan must follow TDD (test-first) as prescribed by AGENTS.md. We need to add failing tests for the partitioning logic before implementing it. This also addresses the reviewer's concern that `models.rs` currently has no tests.

**Files:**
- Modify: `crates/koji-web/src/pages/models.rs` (add `#[cfg(test)]` module)

**What to implement:**
1. Add a `#[cfg(test)]` module at the bottom of models.rs
2. Add a stub helper function:
   ```rust
   fn partition_models_by_loaded(models: Vec<ModelEntry>) -> (Vec<ModelEntry>, Vec<ModelEntry>) {
       // Stub for TDD - returns empty partitions
       (vec![], vec![])
   }
   ```
3. Add tests covering:
   - All loaded → `(n, 0)` - should fail because stub returns `(0, 0)`
   - All unloaded → `(0, n)` - should fail because stub returns `(0, 0)`
   - Mixed → correct split - should fail
   - Empty → `(0, 0)` - should pass
   - Sorts both partitions by `id` ascending - should fail

**Steps:**
- [ ] Add `#[cfg(test)] mod tests { ... }` block at end of models.rs
- [ ] Define the `partition_models_by_loaded` stub function (compiles but returns wrong data)
- [ ] Add test cases for all scenarios mentioned above
- [ ] Run `cargo test -p koji-web` and verify tests fail at runtime (TDD red)
- [ ] Commit with message: "test(models): add failing tests for partition helper"

**Acceptance criteria:**
- [ ] Tests fail at runtime (not compile error) - TDD red
- [ ] Code still builds (stub compiles)

---

## Task 1: Implement partition helper and wire into view

**Context:**
Now that we have failing tests, we implement the partition helper and update the view to render two sections. This task also specifies empty-section handling: sections with no models are skipped entirely (no header above empty grid). The existing "no models configured" empty-state is preserved.

**Files:**
- Modify: `crates/koji-web/src/pages/models.rs`

**What to implement:**
1. Implement `partition_models_by_loaded` to split models into loaded/unloaded vectors
2. Sort each partition by `id` ascending (byte-wise, case-sensitive)
3. Update the view to:
   - Call the partition helper
   - Render `.model-section` wrapper only if the partition is non-empty
   - Add `<h2 class="model-section__title">` header above each section
   - Keep the inner grid structure identical
4. Note: The view iterates `data.models.into_iter()` which moves ownership, so passing to the helper requires no cloning.

**Steps:**
- [ ] Implement `partition_models_by_loaded` function
- [ ] Sort both partitions by `id` ascending using `sort_by(|a, b| a.id.cmp(&b.id))`
- [ ] Update the view! macro to split models into two sections
- [ ] Add conditional rendering: only show section if partition is non-empty
- [ ] Add the `.model-section` div wrappers with `<h2 class="model-section__title">` headers
- [ ] Run `cargo test -p koji-web` and verify all tests pass (TDD green)
- [ ] Run `cargo check -p koji-web` to verify compilation
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(web): split models grid into loaded and unloaded sections"

**Acceptance criteria:**
- [ ] Models are sorted into two sections based on loaded status
- [ ] Each non-empty section has a clear `<h2>` header ("Loaded Models" / "Unloaded Models")
- [ ] Empty sections are not rendered (no header above empty grid)
- [ ] The existing "no models configured" empty-state is preserved
- [ ] Code compiles without errors
- [ ] All unit tests pass
- [ ] Formatting is correct

---

## Task 2: Add CSS for section styling

**Context:**
The HTML structure needs corresponding CSS to provide visual separation between the two sections. This includes margins, borders, and typography for the section headers. This task follows TDD by first adding failing CSS contract tests.

**Files:**
- Modify: `crates/koji-web/tests/css_test.rs` (add tests)
- Modify: `crates/koji-web/style.css` (add rules)

**What to implement:**
1. Add failing CSS contract tests following the existing `style_css_defines_dashboard_models_section_spacing` pattern
2. Add the following CSS rules to section 17 ("Models grid & cards"), after the section comment but before `.models-grid`:
   ```css
   .model-section {
       margin-bottom: 2rem;
   }

   .model-section:last-child {
       margin-bottom: 0;
   }

   .model-section__title {
       font-size: 1.1rem;
       font-weight: 600;
       color: var(--text-secondary);
       margin-bottom: 0.75rem;
       border-bottom: 1px solid var(--border-color);
       padding-bottom: 0.5rem;
   }
   ```

**Steps:**
- [ ] Open `crates/koji-web/tests/css_test.rs` and study the existing CSS contract test pattern
- [ ] Add failing tests for `.model-section`, `.model-section:last-child`, and `.model-section__title` rules
- [ ] Run `cargo test -p koji-web` and verify tests fail (TDD red)
- [ ] Add the CSS rules to `style.css` in section 17 (after the `/* 17. Models grid & cards */` comment)
- [ ] Run `cargo test -p koji-web` and verify tests pass (TDD green)
- [ ] Run `cargo check -p koji-web`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "style(web): add section styling for model grid separation"

**Acceptance criteria:**
- [ ] CSS contract tests fail initially, then pass after CSS is added
- [ ] CSS is syntactically valid
- [ ] Sections have proper spacing between them
- [ ] Section titles are styled consistently with the app's design language
- [ ] No class-name collisions (grep for `.model-section__title` doesn't find existing uses)

---

## Task 3: Final verification

**Context:**
After making all changes, verify that the UI correctly displays the separated sections and that all existing functionality remains intact. This task is verification-only and does not create a commit.

**Files:**
- No file modifications needed

**What to implement:**
1. Build the project and check for any compilation errors
2. Run the full test suite to ensure no regressions
3. Manually verify the visual separation in a running instance

**Steps:**
- [ ] Run `cargo test --workspace` and verify all tests pass
- [ ] Build with `cargo build --release --workspace`
- [ ] Verify the model grid displays two sections with headers
- [ ] Check that models appear in the correct section based on loaded status
- [ ] Check that the "New Model" button still works
- [ ] Check that load/unload actions still work
- [ ] Verify no existing functionality is broken

**Acceptance criteria:**
- [ ] All existing tests pass
- [ ] The model grid displays two sections with headers
- [ ] Models appear in the correct section based on loaded status
- [ ] No existing functionality is broken

---

## Summary

This plan implements a clean visual separation of the model grid into two sections. The changes are minimal and focused:
- 1 file modified for logic (models.rs) - with TDD tests
- 1 file modified for styling (style.css) - with CSS contract tests
- No backend changes or database migrations needed
- No breaking changes to existing functionality

**Commit structure:**
1. `test(models): add failing tests for partition helper` - adds test module + failing tests (builds, tests fail)
2. `feat(web): split models grid into loaded and unloaded sections` - implements logic + passes tests
3. `style(web): add section styling for model grid separation` - adds CSS + contract tests

**Scoping note:** The dashboard's `.dashboard-models` section is intentionally out of scope - it only shows loaded models, so separation isn't needed there.

**Design decisions:**
- Empty sections are skipped entirely (asymmetric: only show section if it has models)
- Sort order is ascending, byte-wise, by `id` (matches backend's `collect_model_statuses` behavior)
- Section titles use `<h2>` for proper document outline accessibility
- Class naming follows BEM-ish convention: `.model-section` and `.model-section__title` (consistent with `.model-card__header`, etc.)
