# [Wizard & Cache Improvements] Plan

**Goal:** Fix the KV cache dropdown UI, add support for advanced quant types (APEX/UD), and implement automatic HF cache cleanup on successful downloads.
**Architecture:** 
- **Frontend (Web):** Update `PullQuantWizard` reactivity and semantic labeling.
- **Backend (Core):** Update quant inference logic and implement post-download cache cleanup.
**Tech Stack:** Rust, Leptos (WASM), `hf-hub`.

---

### Task 1: Fix KV Cache Dropdown (UI/Reactivity)

**Context:**
The user reported that the "dropdown" is actually just a number input. This indicates a failure in the conditional rendering logic in the Leptos component.

**Files:**
- Modify: `crates/koji-web/src/components/pull_quant_wizard.rs`

**What to implement:**
1.  **Introduce State:** Add `let is_custom_context = RwSignal::new(false);` to the `PullQuantWizard` component.
2.  **Refactor `SetContext` View:**
    - Replace the current single `input` with a container.
    - Inside the container, use a `<Show when=move || is_custom_context.get()>` block.
    - **Else branch (Default):** A `<select>` element containing the hardcoded linear increments (`2048`, `4096`, ..., `1048576`) plus a `"Custom..."` option.
    - **When true (Custom):** A numeric `<input type="number">` field.
3.  **Wire Reactivity:**
    - The `on:change` handler for the `select` must:
        - If value is `"custom"`, call `is_custom_context.set(true)`.
        - Otherwise, call `is_custom_context.set(false)` and update the `context_lengths` map with the selected value.
    - The `on:input` handler for the numeric input must update the `context_lengths` map.

**Steps:**
- [ ] Implement the `is_custom_context` signal.
- [ ] Implement the `<Show>` block with the `select` and `input` elements.
- [ ] Wire up the `on:change` and `on:input` handlers.
- [ ] Run `cargo build`
- [ ] Run `cargo fmt --all`
- [ ] **Manual Verification:** Open the wizard, go to "Context", and ensure the dropdown is visible. Select a value, then select "Custom..." to see the input appear.
- [ ] Commit with message: "fix(web): fix KV cache dropdown reactivity"

**Acceptance criteria:**
- [ ] The dropdown is visible by default.
- [ ] Selecting "Custom..." shows the numeric input.
- [ ] Selecting a value from the dropdown updates the context length.

---

### Task 2: Advanced Quant Support (APEX/UD)

**Context:**
The current inference logic is too simple for modern quant types like APEX or UD. We need to extract meaningful names from complex filenames.

**Files:**
- Modify: `crates/koji-core/src/models/pull.rs` (inference logic)
- Modify: `crates/koji-web/src/components/pull_quant_wizard.rs` (display logic)

**What to implement:**
1.  **Refactor `infer_quant_from_filename` in `koji-core`:**
    - Implement a tokenization-based approach. Split the filename (minus `.gguf`) by `-` or `_`.
    - Look for "Semantic Keywords": `Balanced`, `Quality`, `Compact`, `Mini`, `I-Balanced`, `I-Quality`, `I-Compact`, `I-Mini`.
    - Look for "Prefixes": `APEX`, `UD`.
    - Return a structured `QuantInfo` (or similar) that contains both the raw pattern (for backend use) and a `display_name`.
2.  **Update `PullQuantWizard` Display:**
    - Ensure the table uses the new `display_name` provided by the backend (or implement a matching logic in WASM if the backend doesn't provide it).
    - Example: `gemma-4-26B-A4B-APEX-I-Balanced.gguf` $\rightarrow$ `"APEX I-Balanced"`.

**Steps:**
- [ ] Implement tokenization and keyword matching in `koji-core`.
- [ ] Add unit tests in `koji-core` for various filename patterns (Standard, APEX, UD).
- [ ] Update the web component to display the new clean labels.
- [ ] Run `cargo test --package koji-core`
- [ ] Run `cargo build`
- [ ] Run `cargo fmt --all`
- [ ] **Manual Verification:** Search for an APEX model and verify the table shows "APEX I-Balanced" instead of the full filename.
- [ ] Commit with message: "feat(core): add semantic support for APEX and UD quant types"

**Acceptance criteria:**
- [ ] APEX/UD quants show clean, human-readable names in the wizard.
- [ ] Standard quants (Q4_K_M) still work correctly.

---

### Task 3: HF Cache Cleanup

**Context:**
Downloads currently leave duplicates in the HF cache. We need to clean these up after a successful move to the Koji destination.

**Important:** When Koji runs as a Linux service (e.g., via `systemd`), it typically runs as a dedicated user (like `koji`). If the user set `HF_HOME` in their own shell profile (`~/.bashrc`), the service won't see it. Therefore, we cannot rely on `HF_HOME` to find the cache. Instead, we use `api.cache.dir()` which returns the **actual** cache directory being used by `hf-hub`, regardless of how it was configured.

**Files:**
- Modify: `crates/koji-core/src/models/pull.rs` (post-download logic)

**What to implement:**
1.  **Implement `cleanup_hf_cache` function:**
    - This function should take the `source_path` (the HF cache path) and `dest_path` (the final Koji path).
    - **Safety Check:** Verify `dest_path` exists and its size/hash matches the source.
    - **Action:** If safe, attempt to `std::fs::remove_file(source_path)`.
2.  **Integrate into Download Lifecycle:**
    - In the `PullJob` completion logic (after verification), call `cleanup_hf_cache`.
    - Wrap the cleanup in a non-blocking task or error-handled block so a cleanup failure doesn't mark a successful download as "Failed".
3.  **Dynamic Cache Path:**
    - Use `api.cache.dir()` to get the cache directory. This works regardless of `HF_HOME`, service context, or platform.

**Steps:**
- [ ] Implement the `cleanup_hf_cache` function with strict safety checks.
- [ ] Integrate the cleanup into the `PullJob` completion flow.
- [ ] Add a unit test that mocks a successful move and verifies the cache file is deleted.
- [ ] Run `cargo test --package koji-core`
- [ ] Run `cargo build`
- [ ] Run `cargo fmt --all`
- [ ] **Manual Verification:** Download a model, verify it's in the destination, and check that the `.cache/huggingface` entry for that file is gone.
- [ ] Commit with message: "feat(core): implement automatic HF cache cleanup on successful download"

**Acceptance criteria:**
- [ ] Successfully downloaded files are removed from the HF cache.
- [ ] Cleanup failures do not interrupt the user experience.
- [ ] Files are only deleted if the destination is verified.
