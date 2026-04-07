# mmproj Support — Technical Specification

**Goal:** Add support for HuggingFace mmproj (vision projector) files in the model pull workflow and model config.

**Scope:** Frontend wizard UI changes, backend API updates, model config page enhancements.

---

## 1. Data Model Changes

### 1.1 QuantEntry struct (`crates/koji-core/src/models/pull.rs` or `crates/koji-web/src/components/pull_quant_wizard.rs`)

Add `is_mmproj` field to distinguish mmproj files from model quants:

```rust
#[derive(Deserialize, Clone, Debug)]
struct QuantEntry {
    filename: String,
    quant: Option<String>,
    size_bytes: Option<i64>,
    is_mmproj: bool,  // NEW: true if filename matches mmproj*.gguf pattern
}
```

**Helper function:** `is_mmproj_filename(filename: &str) -> bool`
- Returns `true` if filename matches glob pattern `mmproj*.gguf`
- Case-insensitive matching

### 1.2 ModelCard struct (`crates/koji-core/src/models/card.rs`)

Add `mmproj` field to track selected mmproj:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMeta {
    // ... existing fields ...
    pub mmproj: Option<String>,  // NEW: filename of selected mmproj (e.g., "mmproj-F16.gguf")
}
```

---

## 2. Backend Changes

### 2.1 HF List Endpoint (`crates/koji-core/src/proxy/koji_handlers.rs`)

Modify `handle_hf_list_quants` to:
1. Fetch all blobs from HF repo (existing)
2. For each blob, set `is_mmproj` based on filename pattern
3. Return `Vec<QuantEntry>` (no separate mmproj list needed)

**Code change:**
```rust
let mut quants: Vec<QuantEntry> = blobs
    .into_values()
    .map(|b| QuantEntry {
        quant: crate::models::pull::infer_quant_from_filename(&b.filename),
        filename: b.filename,
        size_bytes: b.size,
        is_mmproj: is_mmproj_filename(&b.filename),  // NEW
    })
    .collect();
```

### 2.2 Model Config Save (`crates/koji-core/src/models/card.rs`)

When saving model config, if `mmproj` field is set:
- Verify mmproj file exists in model directory
- If enabled in config, append `--mmproj <path>` to server args

---

## 3. Frontend Wizard Changes

### 3.1 PullQuantWizard Component (`crates/koji-web/src/components/pull_quant_wizard.rs`)

#### 3.1.1 New Signals

```rust
let available_mmprojs = RwSignal::new(Vec::<QuantEntry>::new());
let selected_mmproj_filenames = RwSignal::new(HashSet::<String>::new());
```

#### 3.1.2 New WizardStep

Add `Vision` step between `SelectQuants` and `SetContext`:

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

#### 3.1.3 Step Logic

**On entering `SelectQuants` step:**
- Separate `available_quants` into model quants and mmprojs
- Store mmprojs in `available_mmprojs`
- If `available_mmprojs` is not empty, show "Vision" step

**On entering `Vision` step:**
- Pre-select all available mmprojs (user can deselect)
- Show toast: "Vision projector available: {count} file(s) found"
- Display dropdown with mmproj filenames

**On leaving `Vision` step:**
- Validate at least one mmproj is selected (or allow empty for "no vision")
- Store selected mmproj filenames in `selected_mmproj_filenames`

#### 3.1.4 UI Changes

**Quant Selection Step:**
- Replace text input with dropdown for quant selection
- Show all available quants in dropdown
- Allow multi-select (existing behavior)

**Vision Step (NEW):**
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
            on:change=move |e| {
                // Handle multi-select
                let selected = event_target_value(&e);
                selected_mmproj_filenames.set(selected);
            }
        >
            {move || available_mmprojs.get().into_iter().map(|m| {
                view! { <option value=m.filename.clone()>{m.filename}</option> }
            }).collect::<Vec<_>>()}
        </select>
    </div>

    <div class="form-actions mt-3">
        <button class="btn btn-secondary" on:click=move |_| {
            wizard_step.set(WizardStep::SelectQuants);
        }>"Back"</button>
        <button class="btn btn-primary" on:click=move |_| {
            // Validate and proceed
            wizard_step.set(WizardStep::SetContext);
        }>"Next →"</button>
    </div>
}.into_any(),
```

#### 3.1.5 Download Logic

When building download request:
- Include selected mmprojs in the request
- Backend downloads both model quants and mmprojs together

### 3.2 Toast Notification

Show toast when mmprojs are detected:
```rust
if !available_mmprojs.get().is_empty() {
    web_sys::console::log_1(
        &format!("Vision projector available: {} file(s) found", 
            available_mmprojs.get().len()).into(),
    );
    // TODO: Implement toast component for user notification
}
```

---

## 4. Model Config Page Changes

### 4.1 ModelEditor Component (`crates/koji-web/src/pages/model_editor.rs`)

#### 4.1.1 New Form Fields

**Vision Toggle:**
```rust
let form_vision_enabled = RwSignal::new(false);
```

**MMProj Dropdown:**
```rust
let available_mmprojs_for_select = RwSignal::new(Vec::<String>::new());
let selected_mmproj_for_config = RwSignal::new(String::new());
```

#### 4.1.2 Form Layout

Add Vision section after Quant section:

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
</div>
```

#### 4.1.3 Server Args Generation

When saving model config:
```rust
let mut args = form_args.get().lines().map(|l| l.trim().to_string()).collect::<Vec<_>>();

if form_vision_enabled.get() && !selected_mmproj_for_config.get().is_empty() {
    let mmproj_path = format!("models/{}", selected_mmproj_for_config.get());
    args.push(format!("--mmproj {}", mmproj_path));
}
```

### 4.2 Loading Existing Model

When fetching model config:
- Load `mmproj` field from ModelCard
- Set `form_vision_enabled` based on `mmproj` field presence
- Set `selected_mmproj_for_config` to `mmproj` value
- Populate `available_mmprojs_for_select` from model directory

---

## 5. Edge Cases

### 5.1 Multiple mmprojs
- User can select any subset of available mmprojs
- Only one mmproj can be active in config (dropdown selection)
- Other mmprojs remain on disk for future use

### 5.2 Naming Patterns
- Glob pattern `mmproj*.gguf` matches:
  - `mmproj-F16.gguf`
  - `mmproj-model-name.gguf`
  - `mmproj-Q4_K_M.gguf`
  - Any other `mmproj*.gguf` pattern

### 5.3 No mmprojs Available
- "Vision" step is hidden in wizard
- Vision toggle in config page shows "No mmproj files available"

### 5.4 Model Without mmproj
- Vision toggle is off by default
- No `--mmproj` flag added to server args

---

## 6. Testing

### 6.1 Unit Tests
- `is_mmproj_filename()` function tests
- QuantEntry deserialization with `is_mmproj` field
- ModelCard serialization with `mmproj` field

### 6.2 Integration Tests
- HF repo with mmprojs: verify detection and listing
- HF repo without mmprojs: verify no Vision step shown
- Model config save with/without mmproj
- Server args generation with/without mmproj flag

### 6.3 Manual Smoke Tests
1. Pull model with mmprojs → Vision step appears → Select mmproj → Verify download
2. Pull model without mmprojs → Vision step hidden → No mmproj in config
3. Edit model config → Enable vision → Select mmproj → Verify `--mmproj` flag in args
4. Disable vision → Verify `--mmproj` flag removed from args
5. Multiple mmprojs → Select one → Verify only selected one is active

---

## 7. Files to Modify

### Backend
- `crates/koji-core/src/models/pull.rs` - Add `is_mmproj` field
- `crates/koji-core/src/proxy/koji_handlers.rs` - Update `handle_hf_list_quants`
- `crates/koji-core/src/models/card.rs` - Add `mmproj` field to `ModelMeta`

### Frontend
- `crates/koji-web/src/components/pull_quant_wizard.rs` - Add Vision step
- `crates/koji-web/src/pages/model_editor.rs` - Add Vision toggle and dropdown
- `crates/koji-web/style.css` - Add any new styles for Vision UI

---

## 8. Out of Scope

- mmproj file management (delete, rename)
- mmproj validation (file integrity, compatibility)
- UI for switching between multiple active mmprojs (one at a time only)
- mmproj download progress (handled by existing download UI)
