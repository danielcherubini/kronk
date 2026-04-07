# mmproj Support — Implementation Plan

**Goal:** Add support for HuggingFace mmproj (vision projector) files in the model pull workflow and model config.

**Architecture:** Backend adds `is_mmproj` field to QuantEntry and `mmproj` field to ModelCard. Frontend adds Vision step to pull wizard and Vision toggle to model config page.

**Tech Stack:** Rust + Leptos 0.7 (CSR/WASM), existing koji backend endpoints.

---

## Task 1: Add backend mmproj support to QuantEntry and ModelCard

**Context:**
Before the wizard can detect and download mmproj files, the backend needs to track them. This task adds the `is_mmproj` field to QuantEntry (to distinguish mmproj files from model quants) and the `mmproj` field to ModelCard (to store the selected mmproj filename). These are the foundational data model changes that all other tasks depend on.

**Files:**
- Modify: `crates/koji-core/src/models/pull.rs`
- Modify: `crates/koji-core/src/models/card.rs`
- Modify: `crates/koji-web/src/components/pull_quant_wizard.rs`
- Test: `crates/koji-core/tests/mmproj_detection_test.rs` (new)

**What to implement:**

1. **`crates/koji-core/src/models/pull.rs`** — Add helper function:

```rust
/// Check if a filename matches the mmproj pattern.
/// Matches: mmproj*.gguf (case-insensitive)
pub fn is_mmproj_filename(filename: &str) -> bool {
    let stem = filename.to_lowercase();
    stem.starts_with("mmproj") && stem.ends_with(".gguf")
}
```

2. **`crates/koji-core/src/models/card.rs`** — Add `mmproj` field to `ModelMeta`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMeta {
    // ... existing fields (name, description, etc.) ...
    pub mmproj: Option<String>,  // NEW: filename of selected mmproj (e.g., "mmproj-F16.gguf")
}
```

3. **`crates/koji-web/src/components/pull_quant_wizard.rs`** — Add `is_mmproj` field to `QuantEntry`:

```rust
#[derive(Deserialize, Clone, Debug)]
struct QuantEntry {
    filename: String,
    quant: Option<String>,
    size_bytes: Option<i64>,
    is_mmproj: bool,  // NEW: true if filename matches mmproj*.gguf pattern
}
```

4. **`crates/koji-core/tests/mmproj_detection_test.rs`** — Write tests:

```rust
#[cfg(test)]
mod tests {
    use crate::models::pull::is_mmproj_filename;

    #[test]
    fn test_is_mmproj_filename_positive() {
        assert!(is_mmproj_filename("mmproj-F16.gguf"));
        assert!(is_mmproj_filename("mmproj-model-name.gguf"));
        assert!(is_mmproj_filename("mmproj-Q4_K_M.gguf"));
        assert!(is_mmproj_filename("MMPROJ-F16.GGUF")); // case-insensitive
    }

    #[test]
    fn test_is_mmproj_filename_negative() {
        assert!(!is_mmproj_filename("model-Q4_K_M.gguf"));
        assert!(!is_mmproj_filename("mmproj.bin"));
        assert!(!is_mmproj_filename("model.gguf"));
    }
}
```

**Steps:**
- [ ] Create `crates/koji-core/tests/mmproj_detection_test.rs` with test cases above
- [ ] Run `cargo test --package koji-core mmproj_detection_test`
  - Did it fail with "function or associated item `is_mmproj_filename` not found"? If not, stop and investigate.
- [ ] Add `is_mmproj_filename` function to `crates/koji-core/src/models/pull.rs`
- [ ] Run `cargo test --package koji-core mmproj_detection_test`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Add `mmproj: Option<String>` field to `ModelMeta` struct in `crates/koji-core/src/models/card.rs`
- [ ] Add `is_mmproj: bool` field to `QuantEntry` struct in `crates/koji-web/src/components/pull_quant_wizard.rs`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --workspace`
  - Did all existing tests still pass? If not, stop and investigate.
- [ ] Commit with message: `feat(core): add mmproj support to data models`

**Acceptance criteria:**
- [ ] `is_mmproj_filename` function correctly identifies mmproj files (positive and negative test cases pass)
- [ ] `ModelMeta` struct has `mmproj: Option<String>` field
- [ ] `QuantEntry` struct has `is_mmproj: bool` field
- [ ] All existing tests still pass
- [ ] `cargo build --workspace` and `cargo clippy --workspace -- -D warnings` pass

---

## Task 2: Update backend to set is_mmproj field when listing HF quants

**Context:**
Now that QuantEntry has an `is_mmproj` field, the backend needs to populate it when listing HF repo files. This task updates `handle_hf_list_quants` to call `is_mmproj_filename` for each blob and set the field accordingly. This enables the frontend to distinguish mmproj files from model quants.

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs`
- Test: `crates/koji-core/tests/hf_list_quants_test.rs` (new)

**What to implement:**

1. **`crates/koji-core/src/proxy/koji_handlers.rs`** — Update `handle_hf_list_quants`:

Find the existing code that creates QuantEntry:
```rust
let mut quants: Vec<QuantEntry> = blobs
    .into_values()
    .map(|b| QuantEntry {
        quant: crate::models::pull::infer_quant_from_filename(&b.filename),
        filename: b.filename,
        size_bytes: b.size,
    })
    .collect();
```

Replace with:
```rust
let mut quants: Vec<QuantEntry> = blobs
    .into_values()
    .map(|b| QuantEntry {
        quant: crate::models::pull::infer_quant_from_filename(&b.filename),
        filename: b.filename,
        size_bytes: b.size,
        is_mmproj: crate::models::pull::is_mmproj_filename(&b.filename),  // NEW
    })
    .collect();
```

2. **`crates/koji-core/tests/hf_list_quants_test.rs`** — Write integration test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_hf_list_quants_sets_is_mmproj() {
        // Mock test: verify that QuantEntry is created with is_mmproj field set correctly
        // This test would require mocking the HF API, so we'll do a simpler unit test
        use crate::models::pull::is_mmproj_filename;
        
        let test_cases = vec![
            ("mmproj-F16.gguf", true),
            ("mmproj-model-name.gguf", true),
            ("model-Q4_K_M.gguf", false),
            ("model.gguf", false),
        ];

        for (filename, expected) in test_cases {
            assert_eq!(is_mmproj_filename(filename), expected);
        }
    }
}
```

**Steps:**
- [ ] Create `crates/koji-core/tests/hf_list_quants_test.rs` with test cases above
- [ ] Run `cargo test --package koji-core hf_list_quants_test`
  - Did it fail with "function or associated item `handle_hf_list_quants` not found"? If not, stop and investigate.
- [ ] Update `handle_hf_list_quants` in `crates/koji-core/src/proxy/koji_handlers.rs` to set `is_mmproj` field
- [ ] Run `cargo test --package koji-core hf_list_quants_test`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo build --package koji-core`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
  - Did it pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --package koji-core`
  - Did all existing tests still pass? If not, stop and investigate.
- [ ] Commit with message: `feat(core): set is_mmproj field when listing HF quants`

**Acceptance criteria:**
- [ ] `handle_hf_list_quants` sets `is_mmproj` field for each QuantEntry
- [ ] All existing tests still pass
- [ ] `cargo build --package koji-core` and `cargo clippy --package koji-core -- -D warnings` pass

---

## Task 3: Add Vision step to PullQuantWizard with dropdown UI

**Context:**
Now that the backend can distinguish mmproj files, the wizard needs a new "Vision" step where users can select which mmproj files to download. This step appears between "Select Quants" and "Set Context". The user can select multiple mmprojs (e.g., both F16 and Q4 versions) to download. The step is only shown if mmprojs are detected.

**Files:**
- Modify: `crates/koji-web/src/components/pull_quant_wizard.rs`
- Test: `crates/koji-web/tests/wizard_vision_step_test.rs` (new)

**What to implement:**

1. **Add new signals** in `PullQuantWizard` component:

```rust
let available_mmprojs = RwSignal::new(Vec::<QuantEntry>::new());
let selected_mmproj_filenames = RwSignal::new(HashSet::<String>::new());
```

2. **Add new WizardStep enum variant:**

```rust
enum WizardStep {
    RepoInput,
    LoadingQuants,
    SelectQuants,
    Vision,  // NEW
    SetContext,
    Downloading,
    Done,
}
```

3. **Separate quants from mmprojs** after loading quants:

In the `LoadingQuants` step handler, after `available_quants.set(quants)`:

```rust
let quants_list = available_quants.get();
let mut mmprojs: Vec<QuantEntry> = Vec::new();
let mut model_quants: Vec<QuantEntry> = Vec::new();

for q in quants_list.iter() {
    if q.is_mmproj {
        mmprojs.push(q.clone());
    } else {
        model_quants.push(q.clone());
    }
}

available_quants.set(model_quants);
available_mmprojs.set(mmprojs);
```

4. **Add Vision step view** in the match block:

```rust
WizardStep::Vision => view! {
    <div class="form-card__header">
        <h2 class="form-card__title">"Select Vision Projector"</h2>
        <p class="form-card__desc text-muted">
            "Choose a vision projector file for multimodal support."
        </p>
    </div>

    <div class="form-group">
        <label class="form-label" for="mmproj-select">"Vision Projector"</label>
        <select
            id="mmproj-select"
            class="form-select"
            multiple
            prop:size=move || if available_mmprojs.get().len() > 5 { 5 } else { available_mmprojs.get().len() }
            on:change=move |e| {
                use wasm_bindgen::JsCast;
                let select = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap();
                let selected_filenames: HashSet<String> = select
                    .selected_options()
                    .iter()
                    .map(|option| option.value())
                    .collect();
                selected_mmproj_filenames.set(selected_filenames);
            }
        >
            {move || available_mmprojs.get().into_iter().map(|m| {
                view! { <option value=m.filename.clone()>{m.filename}</option> }
            }).collect::<Vec<_>>()}
        </select>
        <span class="form-hint">"Hold Ctrl/Cmd to select multiple"</span>
    </div>

    {move || {
        if available_mmprojs.get().is_empty() {
            None
        } else {
            Some(view! {
                <div class="alert alert--info mt-2">
                    <span class="alert__icon">"ℹ"</span>
                    <span>"Vision projector available: " {available_mmprojs.get().len()} " file(s) found"</span>
                </div>
            }.into_any())
        }
    }}

    <div class="form-actions mt-3">
        <button class="btn btn-secondary" on:click=move |_| {
            wizard_step.set(WizardStep::SelectQuants);
        }>"Back"</button>
        <button
            class="btn btn-primary"
            prop:disabled=move || selected_mmproj_filenames.get().is_empty()
            on:click=move |_| {
                wizard_step.set(WizardStep::SetContext);
            }
        >"Next →"</button>
    </div>
}.into_any(),
```

5. **Update download logic** to include mmprojs:

In the `SetContext` step, when building the `PullRequest`:

```rust
let mut quants: Vec<QuantRequest> = sel.iter()
    .filter_map(|fname| {
        let entry = quants_list.iter().find(|q| &q.filename == fname)?;
        let ctx = ctx_map.get(fname).copied().unwrap_or(32768);
        Some(QuantRequest {
            filename: fname.clone(),
            quant: entry.quant.clone(),
            context_length: ctx,
        })
    })
    .collect();

// Add selected mmprojs (no context length needed for mmprojs)
let selected_mmprojs: Vec<QuantRequest> = selected_mmproj_filenames
    .get()
    .iter()
    .filter_map(|fname| {
        let entry = available_mmprojs.get().iter().find(|q| &q.filename == fname)?;
        Some(QuantRequest {
            filename: fname.clone(),
            quant: entry.quant.clone(),
            context_length: 0,  // mmprojs don't need context length
        })
    })
    .collect();

quants.extend(selected_mmprojs);
```

6. **Write tests** in `crates/koji-web/tests/wizard_vision_step_test.rs`:

```rust
#[cfg(test)]
mod tests {
    // Test that Vision step is only shown when mmprojs exist
    // Test that multiple mmprojs can be selected
    // Test that mmprojs are included in download request
}
```

**Steps:**
- [ ] Create `crates/koji-web/tests/wizard_vision_step_test.rs` with test cases
- [ ] Run `cargo test --package koji-web --features ssr wizard_vision_step_test`
  - Did it fail with "module or item `wizard_vision_step_test` not found"? If not, stop and investigate.
- [ ] Add `Vision` variant to `WizardStep` enum
- [ ] Add `available_mmprojs` and `selected_mmproj_filenames` signals
- [ ] Add logic to separate quants from mmprojs after loading
- [ ] Add Vision step view in match block
- [ ] Update download logic to include mmprojs
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --workspace`
  - Did all existing tests still pass? If not, stop and investigate.
- [ ] Commit with message: `feat(web): add Vision step to pull wizard for mmproj selection`

**Acceptance criteria:**
- [ ] Vision step appears only when mmprojs are detected
- [ ] Vision step shows dropdown with all available mmprojs
- [ ] Multiple mmprojs can be selected (multi-select)
- [ ] Selected mmprojs are included in download request
- [ ] All existing tests still pass
- [ ] `cargo build --workspace` and `cargo clippy --workspace -- -D warnings` pass

---

## Task 4: Add Vision toggle and mmproj dropdown to model config page

**Context:**
Users need to be able to enable/disable vision support and select which mmproj file to use when editing an existing model. This task adds a Vision toggle checkbox and mmproj dropdown to the model config page. When enabled, the model config saves the selected mmproj filename, which is then used to generate the `--mmproj` flag when starting the server.

**Files:**
- Modify: `crates/koji-web/src/pages/model_editor.rs`
- Test: `crates/koji-web/tests/model_editor_vision_test.rs` (new)

**What to implement:**

1. **Add new signals** in `ModelEditor` component:

```rust
let form_vision_enabled = RwSignal::new(false);
let available_mmprojs_for_select = RwSignal::new(Vec::<String>::new());
let selected_mmproj_for_config = RwSignal::new(String::new());
```

2. **Add Vision section to form layout** (after Quant section):

```rust
<h3 class="form-section-title">"Vision Projector"</h3>
<div class="form-check">
    <input
        id="field-vision-enabled"
        type="checkbox"
        prop:checked=move || form_vision_enabled.get()
        on:change=move |e| {
            use wasm_bindgen::JsCast;
            let checked = e.target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                .map(|el| el.checked())
                .unwrap_or(false);
            form_vision_enabled.set(checked);
        }
    />
    <label class="form-check-label" for="field-vision-enabled">"Enable Vision Projector"</label>
</div>

<div class="form-group" prop:style=move || {
    if form_vision_enabled.get() { "display: block;" } else { "display: none;" }
}>
    <label class="form-label" for="mmproj-select">"Select mmproj File"</label>
    <select
        id="mmproj-select"
        class="form-select"
        prop:value=move || selected_mmproj_for_config.get()
        on:change=move |e| {
            selected_mmproj_for_config.set(event_target_value(&e));
        }
    >
        <option value="">"(none)"</option>
        {move || available_mmprojs_for_select.get().into_iter().map(|m| {
            view! { <option value=m>{m}</option> }
        }).collect::<Vec<_>>()}
    </select>
    <span class="form-hint">"Choose the mmproj file to use for vision support"</span>
</div>
```

3. **Load existing mmproj** when fetching model:

In the Effect that populates signals when detail loads:

```rust
// Load mmproj from model detail
if let Some(mmproj) = &d.mmproj {
    form_vision_enabled.set(true);
    selected_mmproj_for_config.set(mmproj.clone());
    
    // Populate available mmprojs from model directory
    let model_dir = config.models_dir().unwrap_or_else(|_| std::path::PathBuf::from("models"));
    let repo_slug = d.id.replace('/', "--");
    let repo_dir = model_dir.join(&repo_slug);
    
    if repo_dir.exists() {
        let mmprojs: Vec<String> = std::fs::read_dir(&repo_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .starts_with("mmproj")
                    && entry.file_name()
                        .to_string_lossy()
                        .to_lowercase()
                        .ends_with(".gguf")
            })
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        available_mmprojs_for_select.set(mmprojs);
    }
}
```

4. **Update save action** to include mmproj in server args:

In `_save_action`:

```rust
let mut args = form_args
    .get()
    .lines()
    .map(|l| l.trim().to_string())
    .filter(|l| !l.is_empty())
    .collect::<Vec<_>>();

// Add mmproj flag if enabled
if form_vision_enabled.get() && !selected_mmproj_for_config.get().is_empty() {
    let mmproj_path = format!("models/{}/{}", form_id.get(), selected_mmproj_for_config.get());
    args.push(format!("--mmproj {}", mmproj_path));
}
```

5. **Write tests** in `crates/koji-web/tests/model_editor_vision_test.rs`:

```rust
#[cfg(test)]
mod tests {
    // Test that Vision toggle shows/hides mmproj dropdown
    // Test that mmproj is saved to model config
    // Test that --mmproj flag is added to server args
}
```

**Steps:**
- [ ] Create `crates/koji-web/tests/model_editor_vision_test.rs` with test cases
- [ ] Run `cargo test --package koji-web --features ssr model_editor_vision_test`
  - Did it fail with "module or item `model_editor_vision_test` not found"? If not, stop and investigate.
- [ ] Add `form_vision_enabled`, `available_mmprojs_for_select`, `selected_mmproj_for_config` signals
- [ ] Add Vision section to form layout with toggle and dropdown
- [ ] Update model loading effect to populate mmproj fields
- [ ] Update save action to add `--mmproj` flag to args
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --workspace`
  - Did all existing tests still pass? If not, stop and investigate.
- [ ] Commit with message: `feat(web): add Vision toggle and mmproj dropdown to model config`

**Acceptance criteria:**
- [ ] Vision toggle checkbox appears in model config page
- [ ] mmproj dropdown is shown/hidden based on toggle state
- [ ] Existing mmproj is loaded and pre-selected when editing model
- [ ] `--mmproj <path>` flag is added to server args when vision is enabled
- [ ] All existing tests still pass
- [ ] `cargo build --workspace` and `cargo clippy --workspace -- -D warnings` pass

---

## Final Verification

Before merging, run the full check suite:

```bash
make check
cargo build --workspace
```

Verify the PR by:
1. Pull a model with mmprojs → Vision step appears → Select mmproj → Verify download
2. Pull a model without mmprojs → Vision step hidden → No mmproj in config
3. Edit model config → Enable vision → Select mmproj → Verify `--mmproj` flag in args
4. Disable vision → Verify `--mmproj` flag removed from args

---

## Files Summary

**Backend:**
- `crates/koji-core/src/models/pull.rs` - Add `is_mmproj_filename()` function
- `crates/koji-core/src/models/card.rs` - Add `mmproj` field to `ModelMeta`
- `crates/koji-core/src/proxy/koji_handlers.rs` - Set `is_mmproj` in `handle_hf_list_quants`

**Frontend:**
- `crates/koji-web/src/components/pull_quant_wizard.rs` - Add Vision step
- `crates/koji-web/src/pages/model_editor.rs` - Add Vision toggle and dropdown

**Tests:**
- `crates/koji-core/tests/mmproj_detection_test.rs` - Backend detection tests
- `crates/koji-core/tests/hf_list_quants_test.rs` - Backend listing tests
- `crates/koji-web/tests/wizard_vision_step_test.rs` - Wizard UI tests
- `crates/koji-web/tests/model_editor_vision_test.rs` - Config page tests
