# Config Page Redesign Implementation Plan

**Date:** 2026-04-07  
**Author:** Daniel Cherubini  
**Status:** Awaiting Review

---

## Goal
Transform the raw TOML editor config page into a structured, form-based UI with grouped settings sections and inline validation.

## Architecture
The new config page will use Leptos components with reactive signals for state management. It will:
1. Add a structured JSON API (`GET/POST /api/config/structured`) that returns `Json<Config>` for frontend consumption
2. Frontend defines mirror Rust types that can serde-roundtrip via JSON
3. Server merges form changes into loaded Config and persists to TOML
4. UI follows same styling patterns as dashboard.rs and model_editor.rs

**Critical constraint:** WASM build cannot use `toml` crate (gated behind "ssr" feature). Therefore, frontend must use JSON API, not TOML round-trip.

**Callback pattern:** All component callbacks use `leptos::Callback<T>` (not `EventHandler` - fictional type). DOM listeners use inline closures: `on:input=move |e| signal.set(event_target_value(&e))`.

**Mirror types:** Define types in `crates/koji-web/src/types/config.rs`. koji-core cannot be used from WASM (it's an ssr-only dep).

**Important:** Mirror types must contain **every** serde-visible field of the koji-core counterpart. Omitting any field will silently drop user data on POST round-trip. This includes fields the UI doesn't edit.

## Tech Stack
- Leptos 0.7 (Rust web framework)
- gloo_net for HTTP requests (JSON)
- serde_json for JSON serialization
- Mirror Config types in frontend (`crates/koji-web/src/types/config.rs`)
- Existing CSS classes (form-input, form-select, btn-primary, etc.)

---

## Task 0: Add Structured Config JSON API

**Context:**
Per reviewer feedback, the current `/api/config` endpoint works with raw TOML strings, but the WASM build cannot use the `toml` crate (it's gated behind "ssr" feature). To enable frontend parsing/mutation of the Config struct, we need a parallel JSON-based API. This task must be completed before any other config page tasks.

**Files:**
- Modify: `crates/koji-web/src/api.rs` (add new endpoints)
- Modify: `crates/koji-web/src/server.rs` (register new routes)
- Create: `crates/koji-web/src/types/mod.rs` (new module)
- Create: `crates/koji-web/src/types/config.rs` (mirror types for WASM)
- Modify: `crates/koji-web/src/lib.rs` (add `mod types;`)
- Modify: `crates/koji-web/Cargo.toml` (add serde_json and [[test]] entry)
- Note: `/api/config` (TOML) remains unchanged for backward compatibility

**What to implement:**

1. **New endpoints** (`api.rs`):
    - `GET /api/config/structured` → returns `Json<Config>` (full config struct)
    - `POST /api/config/structured` → accepts raw JSON `Config`, validates, persists to TOML, returns success
    - Both endpoints reuse existing `config_path` and `AppState` from `api.rs`
    - Important: Restore `loaded_from` from existing proxy config before persisting (it has `#[serde(skip)]`)
    - Call `sync_proxy_config` for proxy settings hot-reload after successful save
    - **HTTP Status Codes:** All JSON deserialization errors return `400 Bad Request` (Axum's `Json<T>` extractor does not distinguish syntax vs semantic errors). Update docs accordingly.

2. **Mirror types** (`types/config.rs`):
    - Define types: `Config`, `General`, `ProxyConfig`, `BackendConfig`, `Supervisor`, `ModelConfig`, `SamplingParams`, `QuantEntry`, `QuantKind`, `HealthCheck`
    - All types must be `Clone, Serialize, Deserialize`
    - **CRITICAL:** Each type must contain **every** serde-visible field of the koji-core counterpart, including fields the UI doesn't edit
    - **CRITICAL:** Add forward-compat field preservation to prevent data loss from future upstream changes:
      ```rust
      #[derive(Clone, Serialize, Deserialize, Default)]
      pub struct ModelConfig {
          pub backend: String,
          // ...all known fields...
          #[serde(flatten, skip_serializing_if = "Option::is_none")]
          pub extra: Option<serde_json::Map<String, serde_json::Value>>,
      }
      ```
      This pattern preserves unknown fields through round-trip and is the standard serde pattern for forward-compatibility.
    - Use `BTreeMap<String, _>` (not `HashMap`) for `backends`, `models`, `sampling_templates` to guarantee deterministic iteration in UI
    - Mirror `QuantEntry` from `koji-core/src/config/types.rs` (NOT the one in `proxy/koji_handlers.rs`) and `QuantKind`, used by `ModelConfig.quants`. Fields: `file, kind, size_bytes, context_length`.
    - **CRITICAL:** Add default helper functions for all fields that have `#[serde(default = "...")]` in koji-core. Each mirror type must have equivalent default helpers.

3. **Error handling**:
    - 400 Bad Request: JSON deserialization error (Axum returns 400 for all JSON errors, both syntactic and semantic). Update docs from spec's incorrect "422" claim.
    - 500 Internal Server Error: file write error
    - 404 Not Found: config_path not configured

4. **Round-trip test**:
    - Write test in `crates/koji-web/tests/config_structured_test.rs`
    - Test: GET /api/config/structured returns valid JSON
    - Test: POST /api/config/structured persists and round-trips without field loss
    - Test: loaded_from is restored from existing proxy config (not serialized, manually restored)
    - Test: All fields preserved - populate a `ModelConfig` with every field set to a non-default value and assert equality post-round-trip
    - Test: All fields preserved - populate a `Supervisor`, `BackendConfig`, and `ProxyConfig` with all fields and assert equality
    - Test: **Standalone mode** - verify save works when `proxy_config == None` (no panic, graceful handling)
    - Test: **Equivalence** - POST via `/api/config` (TOML), GET via `/api/config/structured`, POST JSON back, compare disk content for equivalence
    - Note: Document that `ModelConfig.profile` field is `skip_serializing` in koji-core and will be silently dropped on structured POST (behavior change from raw TOML endpoint)

5. **Frontend types module**:
    - Create `crates/koji-web/src/types/mod.rs` containing `pub mod config;`
    - Add `mod types;` to `crates/koji-web/src/lib.rs`

**Steps:**
- [ ] Add `[[test]] name = "config_structured_test" path = "tests/config_structured_test.rs" required-features = ["ssr"]` to `crates/koji-web/Cargo.toml`
- [ ] Create `crates/koji-web/src/types/mod.rs` with `pub mod config;`
- [ ] Add `mod types;` to `crates/koji-web/src/lib.rs`
- [ ] Write failing test for structured API round-trip in `crates/koji-web/tests/config_structured_test.rs`
  - Test: GET /api/config/structured returns valid JSON
  - Test: POST /api/config/structured persists and round-trips without field loss
  - Test: loaded_from is restored from existing proxy config (not serialized, manually restored)
  - Test: All ModelConfig fields preserved (populate every field, assert equality post-round-trip)
  - Test: All Supervisor/BackendConfig/ProxyConfig fields preserved
  - Test: Standalone mode - save works when proxy_config is None
  - Test: Equivalence test - /api/config and /api/config/structured produce same disk content
- [ ] Run `cargo test --package koji-web --test config_structured_test`
  - Did it fail with "test failed"? If passed, investigate.
- [ ] Add `serde_json` to `crates/koji-web/Cargo.toml` deps (if not present)
- [ ] Implement `GET /api/config/structured` in `api.rs`
- [ ] Implement `POST /api/config/structured` in `api.rs`
  - Important: Restore `loaded_from` from existing proxy config before persisting (it has `#[serde(skip)]`)
  - Call `sync_proxy_config` for hot-reload after successful save
  - Use `cfg.save_to(&state.config_path.parent()?)` for consistency with existing endpoints
- [ ] Add route in `server.rs`: `.route("/api/config/structured", get(api::get_structured_config).post(api::save_structured_config))`
- [ ] Define mirror types in `crates/koji-web/src/types/config.rs`
  - Note: koji-core cannot be used from WASM (it's an ssr-only dep)
  - Use `BTreeMap<String, _>` for backends, models, sampling_templates
  - Mirror every field from koji-core types (including QuantEntry, QuantKind, HealthCheck)
  - Add `#[serde(flatten, skip_serializing_if = "Option::is_none")] pub extra: Option<serde_json::Map<String, serde_json::Value>>` to prevent data loss from future upstream changes
  - Add default helper functions for all `#[serde(default = "...")]` fields
- [ ] Run `cargo test --package koji-web --test config_structured_test`
  - Did all tests pass? If not, fix failures.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix formatting.
- [ ] Run `cargo check --package koji-web`
  - Did it succeed? If not, fix type errors.
- [ ] Commit with message: "feat: add structured JSON API for config (GET/POST /api/config/structured)"

**Acceptance criteria:**
- [ ] GET /api/config/structured returns valid JSON Config
- [ ] POST /api/config/structured persists and round-trips without field loss
- [ ] models HashMap preserved in round-trip (with all ModelConfig fields)
- [ ] sampling_templates preserved in round-trip
- [ ] loaded_from manually restored from existing proxy config (not serialized)
- [ ] sync_proxy_config called on POST success for proxy hot-reload
- [ ] Round-trip test populates all ModelConfig fields and asserts equality
- [ ] Round-trip test populates all Supervisor/BackendConfig/ProxyConfig fields and asserts equality
- [ ] Standalone mode test passes (no proxy_config)
- [ ] Equivalence test passes (TOML and JSON endpoints produce same disk content)
- [ ] All tests passing
- [ ] No clippy warnings
- [ ] Code formatted

**Note:** This task MUST be completed before Tasks 1-8. All other tasks depend on having a working JSON API.

**CRITICAL FIXES FROM REVIEWER:**
- C1: Add `#[serde(flatten)] extra` field to all mirror types for forward compatibility
- C2: Update docs to reflect Axum returns 400 for all JSON errors (not 422)
- C3: **BEFORE TASK 0** - Either add `health_check_timeout_ms` and `health_check_retries` to `Supervisor` struct in koji-core, OR remove them from `config/koji.toml` example. This is a blocker - do not ship a regression.
- C4: **BEFORE TASK 0** - Reconcile `top_k` range: spec says 1-100, plan says 0-100. Update both documents to match llama.cpp docs (0 disables top-k).
- C5: Specify `cfg.save_to(&state.config_path.parent()?)` for persistence path consistency
- W6: Add default helper functions for all `#[serde(default = "...")]` fields
- W5: Add equivalence test between /api/config and /api/config/structured

---

## Task 1: Create Reusable Form Components

**Context:**
Before building the config page sections, we need reusable form components that handle validation and error display. These will be used across all sections.

**Critical constraint:** Component rendering tests cannot run in WASM without DOM. Tests will be pure-logic unit tests (validation helpers, From impls, serialization) in `#[cfg(test)] mod tests` blocks within each source file.

**Callback pattern:** Use `leptos::Callback<T>` for component-to-component callbacks. DOM listeners use inline closures: `on:input=move |e| signal.set(event_target_value(&e))`.

**Files:**
- Create: `crates/koji-web/src/components/form_input.rs`
- Create: `crates/koji-web/src/components/form_select.rs`
- Create: `crates/koji-web/src/components/form_checkbox.rs`
- Create: `crates/koji-web/src/components/form_validation.rs`
- Modify: `crates/koji-web/src/components/mod.rs` (export new components)

**What to implement:**

1. **FormInput** (`form_input.rs`):
    - Props: `label: String`, `id: String`, `value: Signal<String>`, `placeholder: Option<String>`, `error: Option<String>`, `on_input: Callback<String>`
    - Render: `<label for=id>` + `<input type="text">` with error message below
    - Add `aria-describedby` for accessibility when error exists
    - Add `help_text: Option<String>` prop - **REQUIRED for all numeric fields** (see W4 below)
    - DOM listener: `on:input=move |e| on_input.run(event_target_value(&e))`

2. **FormNumber** (extend FormInput):
    - Props: same as FormInput + `min: Option<f64>`, `max: Option<f64>`, `step: Option<f64>`, `on_change: Callback<String>`
    - Input type="number"
    - Client-side validation: show error if value outside range
    - **CRITICAL:** Every numeric field must have populated `help_text` citing source (see W4)

3. **FormSelect** (`form_select.rs`):
    - Props: `label: String`, `id: String`, `options: Vec<(String, String)>`, `value: Signal<String>`, `error: Option<String>`, `on_change: Callback<String>`
    - Options format: (display_text, value)
    - DOM listener: `on:change=move |e| on_change.run(event_target_value(&e))`

4. **FormCheckbox** (`form_checkbox.rs`):
    - Props: `label: String`, `id: String`, `checked: Signal<bool>`, `on_change: Callback<bool>`
    - Render: `<input type="checkbox">` + `<label for=id>`
    - DOM listener: `on:change=move |e| on_change.run(event_target_checked(&e))`

5. **FormValidation** (`form_validation.rs`):
    - Pure logic functions with unit tests:
      - `fn validate_ip_address(s: &str) -> Result<(), String>` - **Use `std::net::IpAddr::from_str`** (handles v4 + v6, see W2)
      - `fn validate_port(port: u16) -> Result<(), String>`
      - `fn validate_url(s: &str) -> Result<(), String>` - **Use `url::Url::parse`** (add `url` crate to dependencies, see W2)
      - `fn validate_range_f64(value: f64, min: f64, max: f64) -> Result<(), String>`
      - `fn validate_range_u16(value: u16, min: u16, max: u16) -> Result<(), String>`
      - `fn validate_required(s: &str) -> Result<(), String>`
    - Helper: `fn event_target_checked(e: &web_sys::Event) -> bool` for checkbox checked state
    - Helper: `fn event_target_value(e: &web_sys::Event) -> String` for input values
    - Note: No debounce utility - implement debouncing in each section component using `gloo_timers::callback::Timeout` (see I3 for potential extraction)

6. **Accessibility** (baked in from start, not separate task):
    - All inputs: `aria-describedby` linking to error message element
    - Error messages: `role="alert"` or `aria-live="polite"`
    - Labels: `for` attribute matching input `id`
    - **CRITICAL:** Validation states must use **icons + text**, not just color (WCAG 1.4.1). Include checkmark (✓) for valid, error icon (✗) for invalid. See spec fix #9.

**Steps:**
- [ ] Add `url` crate to `crates/koji-web/Cargo.toml` dependencies (see W2)
- [ ] Write failing unit test for FormInput validation in `crates/koji-web/src/components/form_input.rs` (in `#[cfg(test)] mod tests`)
  - Test: validation helpers return correct errors for invalid IP, port, URL
  - Test: `std::net::IpAddr::from_str` handles IPv6 correctly
  - Test: `url::Url::parse` handles edge cases (fragments, query strings)
- [ ] Run `cargo test --package koji-web --features ssr --lib form_input::tests`
  - Did it fail with "test failed"? If passed, investigate.
- [ ] Implement FormInput component with ARIA attributes and validation icons
- [ ] Run `cargo test --package koji-web --features ssr --lib form_input::tests`
  - Did all tests pass? If not, fix failures.
- [ ] Write unit test for FormNumber validation
- [ ] Run `cargo test --package koji-web --features ssr --lib form_input::tests`
  - Did it fail? If passed, investigate.
- [ ] Implement FormNumber with min/max props and help_text
- [ ] Run `cargo test --package koji-web --features ssr --lib form_input::tests`
  - Did all tests pass?
- [ ] Write unit test for FormSelect option mapping
- [ ] Write unit test for FormCheckbox checked state handling
- [ ] Implement FormSelect, FormCheckbox, FormValidation following same TDD pattern
- [ ] Run `cargo test --package koji-web --features ssr --lib form_input form_select form_checkbox`
  - Did all tests pass?
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix formatting issues.
- [ ] Run `cargo check --package koji-web`
  - Did it succeed? If not, fix type errors.
- [ ] **Optional:** Add one `wasm-bindgen-test` smoke test against `FormInput` (see W3)
  - Even a single DOM test proves infrastructure exists for follow-up coverage
- [ ] Commit with message: "feat: add reusable form components with ARIA and validation"

**Acceptance criteria:**
- [ ] FormInput renders with label, input, and optional error message
- [ ] FormNumber validates min/max range client-side
- [ ] FormSelect renders dropdown with custom options
- [ ] FormCheckbox renders checkbox with label
- [ ] All validation helpers return Result<(), String>
- [ ] All components export from `components/mod.rs`
- [ ] All numeric fields have populated help_text citing source (see W4)
- [ ] Validation states include icons (✓/✗), not just color
- [ ] All tests passing
- [ ] Code formatted with `cargo fmt`
- [ ] No clippy warnings

**CRITICAL FIXES FROM REVIEWER:**
- W2: Use `std::net::IpAddr::from_str` for IP, `url::Url::parse` for URLs (add `url` crate)
- W3: Add at least one wasm-bindgen-test smoke test (optional but recommended)
- W4: Every numeric field must have populated help_text - add acceptance criterion
- W8: Validation states must use icons + text, not just color (WCAG 1.4.1)

---

## Task 2: Extract SamplingField Component

**Context:**
`SamplingField` already exists in `model_editor.rs:67` but needs to be extracted to a reusable component. Per the reviewer's critical feedback, SamplingParams fields are Option<T> with skip_serializing. We cannot use hardcoded defaults (1.0, 40, etc.) because that would emit unnecessary CLI args. We need the enabled+value pattern to only serialize fields the user explicitly enables.

**Files:**
- Modify: `crates/koji-web/src/components/sampling_field.rs` (create new file)
- Modify: `crates/koji-web/src/components/mod.rs` (export new component)
- Read: `crates/koji-web/src/pages/model_editor.rs:67-71` (existing SamplingField)
- Read: `crates/koji-web/src/pages/model_editor.rs:130` (existing parsing logic)
- Read: `crates/koji-core/src/profiles.rs` (for SamplingParams structure)

**What to implement:**

1. **SamplingField struct** (extract from model_editor.rs):
    ```rust
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SamplingField {
        pub enabled: bool,
        pub value: String,  // Store as string to handle empty vs 0
    }
    ```

2. **Mapping functions**:
    - `impl From<&SamplingParams> for SamplingField` - convert from backend type (per-field)
    - **CRITICAL:** Define explicit `From<Option<f64>> for SamplingField` with loading rule (see W11):
      ```rust
      impl From<Option<f64>> for SamplingField {
          fn from(opt: Option<f64>) -> Self {
              match opt {
                  Some(v) => SamplingField { enabled: true, value: v.to_string() },
                  None    => SamplingField { enabled: false, value: String::new() },
              }
          }
      }
      ```
      This ensures loading a template with `Some(0.3)` shows the toggle ON with value "0.3", while `None` shows toggle OFF.
    - `impl From<&SamplingField> for Option<u32>` for top_k specifically (use u32, not u64)
    - `impl From<&SamplingField> for Option<f64>` for other numeric fields (temperature, top_p, etc.)
    - Note: top_k is u32 in SamplingParams, all others are f64 - must handle type conversion

3. **UI Component**:
    - Props: `label: String`, `field: SamplingField`, `error: Option<String>`, `on_enable: Callback<bool>`, `on_change: Callback<String>`
    - Render: Checkbox for "Enabled" toggle + number input for value
    - When disabled, show value input as disabled/grayed out
    - When enabled, show value input as active
    - DOM listeners: `on:change=move |e| on_enable.run(event_target_checked(&e))` for toggle, `on:input=move |e| on_change.run(event_target_value(&e))` for value

**Steps:**
- [ ] Write failing unit test for SamplingField serialization in `crates/koji-web/src/components/sampling_field.rs` (in `#[cfg(test)] mod tests`)
  - Test: `Option::<f64>::from(&field)` returns `None` when `enabled == false`, regardless of value
  - Test: `Option::<f64>::from(&field)` returns `Some(value)` when `enabled == true`
  - Test: top_k conversion handles u32 correctly (not u64)
  - Test: `From::from(Some(0.3))` yields `enabled == true` (loading rule)
  - Test: `From::from(None)` yields `enabled == false` (loading rule)
  - Test: Invalid top_k values (overflow u32) return validation error
- [ ] Run `cargo test --package koji-web --features ssr --lib sampling_field::tests`
  - Did it fail? If passed, investigate.
- [ ] Extract `SamplingField` struct from `model_editor.rs:67` to `crates/koji-web/src/components/sampling_field.rs`
- [ ] Implement From traits for conversion (handle u32 for top_k separately)
- [ ] Port parsing logic from `model_editor.rs:130` (use u32 for top_k, not u64)
- [ ] **CRITICAL:** Add validation test for top_k overflow - `top_k = "99999999999"` (overflows u32) should return validation error, not silently fail (see C6)
- [ ] Run `cargo test --package koji-web --features ssr --lib sampling_field::tests`
  - Did all tests pass?
- [ ] Implement SamplingField UI component with toggle
- [ ] Run `cargo test --package koji-web --features ssr --lib sampling_field::tests`
  - Did all tests pass?
- [ ] Update `model_editor.rs` to import `SamplingField` from new location
- [ ] Run `cargo test --package koji-web --features ssr`
  - Did all tests pass? (verify no regression to model_editor)
- [ ] Run `cargo fmt --all`
  - Did it succeed?
- [ ] Run `cargo check --package koji-web`
  - Did it succeed?
- [ ] Commit with message: "feat: extract SamplingField component with enabled toggle"

**Acceptance criteria:**
- [ ] SamplingField serializes to None when enabled=false
- [ ] SamplingField serializes to Some(value) when enabled=true
- [ ] UI shows enabled toggle + value input
- [ ] Disabled state visually indicated (grayed out input)
- [ ] top_k uses u32 (not u64)
- [ ] Loading rule implemented: `Some(v)` → toggle ON, `None` → toggle OFF
- [ ] top_k overflow returns validation error (not silent failure)
- [ ] All tests passing
- [ ] No regression in model_editor.rs
- [ ] No clippy warnings

**CRITICAL FIXES FROM REVIEWER:**
- C6: Add validation test for top_k overflow (u32 vs u64)
- W11: Explicitly define `From<Option<f64>> for SamplingField` with loading rule

---

## Task 3: Create Side Navigation Component

**Context:**
The config page needs a left sidebar navigation to switch between sections. This component will be reusable and follow the existing styling patterns from nav.rs and dashboard.rs.

**Files:**
- Create: `crates/koji-web/src/components/config_nav.rs`
- Modify: `crates/koji-web/src/components/mod.rs`

**What to implement:**

1. **ConfigNav component**:
    - Props: `active_section: String`, `on_select: Callback<String>` (section name)
    - Sections: ["General", "Backends", "Supervisor", "Sampling Templates"]
    - Render: Vertical list of links with active state styling
    - Mobile: Hide behind hamburger menu (use existing topbar pattern from nav.rs)
    - Accessibility: Add `aria-current="page"` to active section
    - Data constant: `pub const SECTIONS: &[&str] = &["General", "Backends", "Supervisor", "Sampling Templates"];`

2. **Mobile Navigation (CRITICAL for W8)**:
    - Hamburger menu toggle must include `aria-expanded` attribute
    - **Focus trap** when menu is open - focus must not escape until menu closed
    - Close on outside click
    - Show active section indicator even when sidebar closed (e.g., in header or as visual cue)
    - **Mobile breakpoint:** Define explicit breakpoint (e.g., `@media (max-width: 768px)`)

3. **Styling**:
    - Use existing `.nav-item`, `.nav-item--active` classes if available
    - If not, create `.config-nav`, `.config-nav__item`, `.config-nav__item--active`
    - Desktop: Always visible (width ~200px)
    - Mobile: Hidden by default, toggle via hamburger

**Steps:**
- [ ] Write failing unit test for ConfigNav in `crates/koji-web/src/components/config_nav.rs` (in `#[cfg(test)] mod tests`)
  - Test: SECTIONS constant has exactly 4 items with correct values
  - Test: active section has aria-current="page"
  - Test: callback is wired (not anchor links with href)
- [ ] Run `cargo test --package koji-web --features ssr --lib config_nav::tests`
  - Did it fail? If passed, investigate.
- [ ] Implement ConfigNav component with ARIA attributes
- [ ] **CRITICAL:** Add mobile hamburger menu with:
  - `aria-expanded` attribute on toggle button
  - Focus trap implementation when menu is open
  - Close on outside click behavior
  - Active section indicator visible even when sidebar closed
- [ ] Define mobile breakpoint in CSS (e.g., `@media (max-width: 768px)`)
- [ ] Run `cargo fmt --all`
  - Did it succeed?
- [ ] Run `cargo check --package koji-web`
  - Did it succeed?
- [ ] Commit with message: "feat: add side navigation component with ARIA and mobile support"

**Acceptance criteria:**
- [ ] ConfigNav renders 4 section links
- [ ] Active section has visual distinction and aria-current="page"
- [ ] Clicking link triggers on_select callback with section name
- [ ] Mobile hamburger menu with `aria-expanded` attribute
- [ ] Focus trap implemented when mobile menu is open
- [ ] Close on outside click works
- [ ] Active section indicator visible on mobile even when sidebar closed
- [ ] All tests passing
- [ ] No clippy warnings

**CRITICAL FIXES FROM REVIEWER:**
- W8: Hamburger menu needs `aria-expanded` and focus trap
- W8: Mobile active-section indicator when nav closed
- Spec fix #10: Define mobile breakpoint explicitly

---

## Task 4: Create General Section Component

**Context:**
The General section combines core app settings (log_level, models_dir, logs_dir) with Proxy settings. This is the first section to implement and will serve as a template for other sections.

**Files:**
- Create: `crates/koji-web/src/pages/config_sections/general.rs`
- Create: `crates/koji-web/src/pages/config_sections/mod.rs`
- Modify: `crates/koji-web/src/pages/mod.rs` (export new module)

**What to implement:**

1. **GeneralSection component**:
    - Props: `config: Config`, `on_change: Callback<Config>`
    - Subsections:
      - General Settings: log_level (select), models_dir (input), logs_dir (input)
      - Proxy Settings (with visual separator): enabled (checkbox), host (input), port (number), idle_timeout_secs (number), startup_timeout_secs (number), circuit_breaker_threshold (number), circuit_breaker_cooldown_seconds (number), metrics_retention_secs (number)
    - Note: Use exact field names from ProxyConfig struct in types.rs

2. **Validation rules**:
    - log_level: must be one of ["debug", "info", "warn", "error"]
    - port: 1-65535
    - All number inputs: range validation
    - host: IP address format validation (use `std::net::IpAddr::from_str`)
    - URLs: valid URL format (use `url::Url::parse`)

3. **Data flow**:
    - On any field change, emit ConfigChange event with updated values
    - Parent component aggregates changes and saves via API

4. **Data constant for field names**: `pub fn general_field_names() -> &'static [&'static str]` to verify field names at compile time

5. **Help Text (CRITICAL for W4)**: Every numeric field must have populated `help_text` citing source. Add this table to the task:

   | Field | Range | Source Justification |
   |-------|-------|---------------------|
   | port | 1-65535 | Valid TCP port range (IANA) |
   | idle_timeout_secs | 1-3600 | llama.cpp default: 300s |
   | startup_timeout_secs | 1-600 | Backend startup reasonable limit |
   | circuit_breaker_threshold | 1-100 | Reasonable failure count before circuit opens |
   | circuit_breaker_cooldown_secs | 1-300 | 60s default in spec |
   | metrics_retention_secs | 60-604800 | 1 day to 1 week range |

**Steps:**
- [ ] Write failing unit test for GeneralSection field mapping in `crates/koji-web/src/pages/config_sections/general.rs` (in `#[cfg(test)] mod tests`)
  - Test: field names match ProxyConfig struct exactly
  - Test: validation rules applied correctly
  - Test: general_field_names() returns correct list
- [ ] Run `cargo test --package koji-web --features ssr --lib general::tests`
  - Did it fail? If passed, investigate.
- [ ] Implement GeneralSection component with exact field names
- [ ] Add validation for each field type
- [ ] **CRITICAL:** Add populated `help_text` to all numeric fields citing source (see table above)
- [ ] Run `cargo test --package koji-web --features ssr --lib general::tests`
  - Did all tests pass?
- [ ] Add debounced validation using gloo_timers::callback::Timeout (500ms delay)
- [ ] Run `cargo fmt --all`
  - Did it succeed?
- [ ] Run `cargo check --package koji-web`
  - Did it succeed?
- [ ] Commit with message: "feat: add General section with Proxy settings (exact field names)"

**Acceptance criteria:**
- [ ] GeneralSection renders all 11 fields (3 General + 8 Proxy)
- [ ] Proxy section visually separated with header
- [ ] All fields have inline validation
- [ ] Validation errors shown below fields
- [ ] All numeric fields have populated help_text citing source
- [ ] Debounced validation (500ms)
- [ ] All tests passing
- [ ] No clippy warnings

**CRITICAL FIXES FROM REVIEWER:**
- W4: Every numeric field must have populated help_text - add acceptance criterion
- Spec fix #12: Each range must be justified with source (llama.cpp docs, backend defaults, etc.)

---

## Task 5: Create Backends Section Component

**Context:**
The Backends section displays a list of configured backends (llama_cpp, ik_llama, etc.) with editable fields. No add/remove functionality for now - only edit existing.

**Files:**
- Create: `crates/koji-web/src/pages/config_sections/backends.rs`
- Modify: `crates/koji-web/src/pages/config_sections/mod.rs`

**What to implement:**

1. **BackendCard component**:
    - Props: `name: String`, `backend: BackendConfig`, `on_change: Callback<(String, BackendConfig)>` (emits backend name + full config)
    - Parent `BackendsSection` receives the tuple and updates the correct entry in its BTreeMap
    - Fields: path (input), default_args (textarea, one per line), health_check_url (input), version (input)
    - Collapsible: Show/hide fields with expand button

2. **BackendsSection component**:
    - Props: `backends: BTreeMap<String, BackendConfig>`, `on_change: Callback<BTreeMap<String, BackendConfig>>`
    - Render: List of BackendCard components
    - Show count: "3 backends configured"
    - No add/remove buttons (per spec)
    - Use BTreeMap for deterministic iteration order

3. **default_args handling (CRITICAL for W12)**:
    - Store as Vec<String> internally
    - Display as newline-separated textarea
    - Parse on save with edge case handling:
      ```rust
      fn parse_args(text: &str) -> Vec<String> {
          text.lines()
              .map(|s| s.trim())
              .filter(|s| !s.is_empty())
              .collect()
      }
      ```
    - **Help text for textarea:** "Enter one argument per line. Each line becomes a separate CLI token. For `--flag value`, use two lines."
    - Document this limitation clearly in UI (spec fix #7)

4. **Validation**:
    - URL validation for health_check_url using `url::Url::parse` from Task 1

5. **Data constant for field names**: `pub fn backend_card_field_names() -> &'static [&'static str]` to verify field names at compile time

**Steps:**
- [ ] Write failing unit test for default_args parsing in `crates/koji-web/src/pages/config_sections/backends.rs` (in `#[cfg(test)] mod tests`)
  - Test: newline-separated string splits into Vec<String>
  - Test: empty string produces empty Vec
  - Test: trailing newline handled correctly
  - Test: empty lines are filtered out
  - Test: whitespace is trimmed
- [ ] Run `cargo test --package koji-web --features ssr --lib backends::tests`
  - Did it fail? If passed, investigate.
- [ ] Implement BackendCard component with collapsible fields
- [ ] Implement BackendsSection with BTreeMap iteration (sorted keys for deterministic order)
- [ ] Add URL validation for health_check_url using validate_url helper
- [ ] **CRITICAL:** Add help text to default_args textarea explaining one-arg-per-line format
- [ ] Run `cargo test --package koji-web --features ssr --lib backends::tests`
  - Did all tests pass?
- [ ] Run `cargo fmt --all`
  - Did it succeed?
- [ ] Run `cargo check --package koji-web`
  - Did it succeed?
- [ ] Commit with message: "feat: add Backends section with editable cards"

**Acceptance criteria:**
- [ ] BackendsSection renders list of backend cards
- [ ] Each card is collapsible
- [ ] default_args displayed as newline-separated textarea
- [ ] default_args has help text explaining one-arg-per-line format
- [ ] Parsing filters empty lines and trims whitespace
- [ ] Changes emit BackendChange events
- [ ] All tests passing
- [ ] No clippy warnings

**CRITICAL FIXES FROM REVIEWER:**
- W12: Update parsing to filter empty lines and trim whitespace
- W12: Add explicit help text explaining one-arg-per-line format
- Spec fix #7: Document the tokenization limitation in UI

---

## Task 6: Create Supervisor Section Component

**Context:**
The Supervisor section has 4 simple fields for process management. This is a straightforward form section similar to General but without the Proxy subsection.

**CRITICAL BLOCKER (C3):** Before implementing this task, you MUST resolve the `health_check_timeout_ms` and `health_check_retries` issue:
- These fields exist in `config/koji.toml` (lines 19-20) but NOT in `Supervisor` struct
- They will be silently dropped on first save through the new UI
- **ACTION REQUIRED:** Either:
  - **Option A (preferred):** Add `health_check_timeout_ms: u64` and `health_check_retries: u32` to `Supervisor` struct in `koji-core/src/config/types.rs`
  - **Option B:** Remove these fields from `config/koji.toml` example config to avoid confusion

**Files:**
- Create: `crates/koji-web/src/pages/config_sections/supervisor.rs`
- Modify: `crates/koji-web/src/pages/config_sections/mod.rs`

**What to implement:**

1. **SupervisorSection component**:
    - Props: `supervisor: Supervisor`, `on_change: Callback<Supervisor>`
    - Fields (from Supervisor struct in types.rs):
      - restart_policy (select): ["always", "on-failure", "never"]
        - **Action:** Verify usage by checking `grep -rn 'restart_policy' crates/koji-core/src`
        - If unused, add help text "Not currently used by supervisor"
      - max_restarts (number): 0-1000 (0 = exit on first failure)
      - restart_delay_ms (number): 100-60000
      - health_check_interval_ms (number): 1000-60000
    - **CRITICAL:** After C3 is resolved:
      - If Option A chosen: Add `health_check_timeout_ms` and `health_check_retries` fields to UI
      - If Option B chosen: Document in help text that these fields exist in example config but are not managed by UI

2. **Validation**:
    - All number fields with range validation
    - restart_policy: dropdown with exact values (even if unused, keep for consistency)

3. **Data constant for field names**: `pub fn supervisor_field_names() -> &'static [&'static str]` to verify field names at compile time

**Steps:**
- [ ] **BLOCKER:** Resolve C3 - Either add fields to Supervisor struct OR remove from example config
- [ ] Write failing unit test for SupervisorSection field mapping in `crates/koji-web/src/pages/config_sections/supervisor.rs` (in `#[cfg(test)] mod tests`)
  - Test: supervisor_field_names() returns correct list (includes health_check fields if C3 Option A chosen)
  - Test: field names match Supervisor struct in types.rs exactly
  - Test: restart_policy help text if unused
- [ ] Run `cargo test --package koji-web --features ssr --lib supervisor::tests`
  - Did it fail? If passed, investigate.
- [ ] Implement SupervisorSection component
- [ ] Verify restart_policy usage by checking process.rs, supervisor.rs (grep: `grep -rn 'restart_policy' crates/koji-core/src`)
  - If unused, add help text "Not currently used by supervisor"
- [ ] **CRITICAL:** If C3 Option A chosen, add health_check_timeout_ms and health_check_retries fields
- [ ] Run `cargo test --package koji-web --features ssr --lib supervisor::tests`
  - Did all tests pass?
- [ ] Run `cargo fmt --all`
  - Did it succeed?
- [ ] Run `cargo check --package koji-web`
  - Did it succeed?
- [ ] Commit with message: "feat: add Supervisor section with exact struct fields"

**Acceptance criteria:**
- [ ] C3 is resolved before implementation (fields added to struct OR removed from example)
- [ ] SupervisorSection renders all fields present in Supervisor struct
- [ ] restart_policy dropdown has correct options
- [ ] Number fields have range validation
- [ ] Help text added for deprecated/unused fields
- [ ] All tests passing
- [ ] No clippy warnings

**CRITICAL FIXES FROM REVIEWER:**
- **C3 (BLOCKER):** health_check_timeout_ms and health_check_retries must be addressed BEFORE implementation
- Spec fix #3: Either add fields to Supervisor struct OR remove from example config
- Do not ship a regression that silently deletes user config fields

---

## Task 7: Create Sampling Templates Section Component

**Context:**
The Sampling Templates section is the most complex - it's a card-based grid where users can add, edit, and delete custom templates. Built-in templates (coding, chat, analysis, creative) cannot be deleted.

**Files:**
- Create: `crates/koji-web/src/pages/config_sections/sampling_templates.rs`
- Modify: `crates/koji-web/src/pages/config_sections/mod.rs`

**What to implement:**

1. **SamplingTemplateCard component**:
    - Props: `name: String`, `params: SamplingParams`, `is_builtin: bool`, `on_edit: Callback<String>`, `on_delete: Callback<String>`
    - Display: template name, temp, top_p values
    - Buttons: Edit (opens editor), Delete (×) - delete disabled for builtins
    - Grid layout: responsive (1 col mobile, 2 col tablet, 3-4 col desktop)
    - Note: Built-in templates are hardcoded: `["coding", "chat", "analysis", "creative"]`
      - These match `Profile::all()` in profiles.rs
      - Hardcode this list client-side (no server round-trip needed)
      - **CRITICAL:** Add test to sync hardcoded list with `Profile::all()` (see I5)
    - Data constant: `pub const BUILTIN_TEMPLATES: &[&str] = &["coding", "chat", "analysis", "creative"];`

2. **SamplingTemplateEditor modal/component**:
    - Define struct: `#[derive(Debug, Clone)] pub struct SamplingTemplateSave { pub original_name: Option<String>, pub new_name: String, pub params: SamplingParams }`
    - Props: `template: Option<(String, SamplingParams)>`, `existing_names: Vec<String>`, `on_save: Callback<SamplingTemplateSave>`, `on_close: Callback<()>`
    - Fields: name (text), temperature (number with enabled toggle), top_k (number with toggle), top_p (number with toggle), min_p (number with toggle), presence_penalty (number with toggle), frequency_penalty (number with toggle), repeat_penalty (number with toggle)
    - Use SamplingField pattern from Task 2 for all numeric fields
    - Validation: name required, unique (unless editing same name), ranges for numbers
    - Explicit ranges:
      - temperature: 0.0-2.0
      - top_k: 0-100 (u32) - **CRITICAL:** Use 0-100 to match llama.cpp (C4)
      - top_p, min_p: 0.0-1.0
      - penalties: -2.0 to 2.0
    - **CRITICAL (W7):** Built-in template rename protection:
      - Name field is **read-only** when `is_builtin == true`
      - For custom templates, on rename attempt show confirmation: "Models referencing this template will break."
      - Add unit tests for both behaviors

3. **SamplingTemplatesSection component**:
    - Props: `templates: BTreeMap<String, SamplingParams>`, `on_add: Callback<()>, on_edit: Callback<String>, on_delete: Callback<String>, on_save: Callback<SamplingTemplateSave>`
    - Render: Grid of cards + "Add Template" button
    - State: track which template is being edited (for modal)
    - Use BTreeMap for deterministic iteration order
    - **CRITICAL:** Add test sync with `Profile::all()` (I5)

4. **Data constant for card summary**: `pub fn card_summary(params: &SamplingParams) -> (Option<f64>, Option<f64>)` returning (temperature, top_p)

5. **Use existing Modal component (I7):**
    - Do NOT create new modal - reuse `crates/koji-web/src/components/modal.rs`
    - Both SamplingTemplateEditor and Reload confirmation should use existing component

**Steps:**
- [ ] Write failing unit test for SamplingTemplateCard in `crates/koji-web/src/pages/config_sections/sampling_templates.rs` (in `#[cfg(test)] mod tests`)
  - Test: BUILTIN_TEMPLATES constant has exactly 4 items with correct values
  - Test: card_summary() returns correct (temperature, top_p) tuple
  - Test: delete button disabled for built-in names
- [ ] Run `cargo test --package koji-web --features ssr --lib sampling_templates::tests`
  - Did it fail? If passed, investigate.
- [ ] Implement SamplingTemplateCard
- [ ] Run `cargo test --package koji-web --features ssr --lib sampling_templates::tests`
  - Did all tests pass?
- [ ] Write failing unit test for SamplingTemplateEditor in same file
  - Test: SAMPLING_FIELD_NAMES constant contains all 7 SamplingParams fields
  - Test: name validation (required, unique)
  - Test: built-in templates **cannot be deleted**
  - Test: **built-in templates cannot be renamed** (name field read-only)
  - Test: custom template rename shows confirmation warning
  - Test: numeric ranges validated
- [ ] Run `cargo test --package koji-web --features ssr --lib sampling_templates::tests`
  - Did it fail? If passed, investigate.
- [ ] Implement SamplingTemplateEditor with SamplingField components
- [ ] **CRITICAL:** Make name field read-only for built-in templates
- [ ] **CRITICAL:** Add rename confirmation for custom templates
- [ ] Implement SamplingTemplatesSection with grid layout
- [ ] Add "Add Template" functionality (opens empty editor)
- [ ] Add delete functionality (with confirmation for non-builtin)
- [ ] **CRITICAL:** Use existing `Modal` component (do not create new modal)
- [ ] **CRITICAL:** Add test syncing BUILTIN_TEMPLATES with `Profile::all()`
- [ ] Run `cargo test --package koji-web --features ssr --lib sampling_templates::tests`
  - Did all tests pass?
- [ ] Run `cargo fmt --all`
  - Did it succeed?
- [ ] Run `cargo check --package koji-web`
  - Did it succeed?
- [ ] Commit with message: "feat: add Sampling Templates section with built-in protection"

**Acceptance criteria:**
- [ ] SamplingTemplatesSection renders grid of template cards
- [ ] Built-in templates cannot be deleted
- [ ] Built-in templates cannot be renamed (name field read-only)
- [ ] Custom template rename shows confirmation warning
- [ ] "Add Template" button opens empty editor
- [ ] Edit button opens editor with template data
- [ ] Delete button removes template (with confirmation)
- [ ] All sampling fields use enabled toggle pattern
- [ ] Name validation (required, unique)
- [ ] Numeric ranges validated (top_k: 0-100)
- [ ] Uses existing Modal component (not new modal)
- [ ] BUILTIN_TEMPLATES sync test with `Profile::all()` passes
- [ ] All tests passing
- [ ] No clippy warnings

**CRITICAL FIXES FROM REVIEWER:**
- W7: Built-in template rename disabled - name field must be read-only
- W7: Custom template rename shows warning about model references
- C4: top_k range must be 0-100 (spec says 1-100, plan says 0-100 - reconcile to 0-100)
- I5: Add test syncing BUILTIN_TEMPLATES with `Profile::all()`
- I7: Use existing Modal component, don't create new one

---

## Task 8: Create Main Config Page

**Context:**
Now we assemble all the pieces into a single config page that ties together navigation, sections, and the save bar. This page will load config from API, display sections, and handle saving.

**Files:**
- Modify: `crates/koji-web/src/pages/config_editor.rs` (replace existing implementation)
- Modify: `crates/koji-web/src/pages/mod.rs` (ensure ConfigEditor is exported)
- Read: `crates/koji-web/src/api.rs` (for GET/POST /api/config/structured)

**What to implement:**

1. **ConfigEditor page component**:
    - Define enum `#[derive(Debug, Clone, PartialEq)] pub enum SaveStatus { Idle, Saving, Validating, Success(String), Error(String) }` in config_editor.rs
      - **I4:** Add `Validating` state for brief validation phase before POST
    - State:
      - `config: RwSignal<Option<Config>>` - loaded config
      - `pending_changes: RwSignal<Config>` - accumulated form changes
      - `save_status: RwSignal<SaveStatus>` - idle, validating, saving, success, error
      - `active_section: RwSignal<String>` - current section
    - **W10:** Optimize state to avoid full Config clones on every keystroke:
      - Consider section-scoped signals (`RwSignal<General>`, `RwSignal<HashMap<String, BackendConfig>>`, …)
      - Or use Leptos's `update()` instead of `set()` to avoid clone
    - Effects:
      - On load: GET /api/config/structured, parse JSON, populate config signal
      - On section nav click: scroll to section, update active_section
      - On form change: update pending_changes
    - Layout:
      - Left sidebar: ConfigNav component
      - Right content: SectionContainer for each section
      - Bottom: SaveBar with "Save All" and "Reload" buttons

2. **SectionContainer component**:
    - Props: `title: String`, `description: AnyView` (not String - allows inline links, see I6), `children: View`
    - Render: Section header, description, form fields
    - Scroll target: id attribute for navigation

3. **SaveBar component**:
    - Props: `on_save: Callback<()>, on_reload: Callback<()>, save_status: SaveStatus`
    - Buttons: "Save All" (primary), "Reload" (secondary)
    - Status banner: success/error message
    - Sticky footer positioning
    - **CRITICAL (W8):** Add `padding-bottom` to main content so sticky bar doesn't obscure last field

4. **Save flow (CRITICAL for W9)**:
    - On "Save All" click:
      1. Validate all sections
      2. If any invalid: abort save, scroll to first error, focus it, announce via `aria-live`
      3. If all valid: set `save_status = Validating`, run synchronous validation, then POST
    - Request body: raw JSON Config (not wrapped in `{ "config": ... }`)
    - On success (200): show "Saved!" message
    - **W1 (CRITICAL):** Per-section diff for restart banner:
      - Capture `loaded_config` separately from `pending_changes`
      - After save, compare `loaded.general/backends/supervisor/sampling_templates/models` vs saved version
      - If any **non-proxy** section changed, show "Restart may be required" banner
      - If only proxy changed, show "Proxy settings hot-reloaded" message (no restart needed)
      - Derive `PartialEq` on relevant structs for comparison
    - On error (400): show validation errors from server (JSON deserialization error)
    - On error (500): show server error message "Server error, please try again"
    - "Reload" button: confirm dialog "Discard unsaved changes and reload from disk?", then GET /api/config/structured (discards changes)
    - Note: Proxy settings hot-reload via sync_proxy_config, other settings require service restart
    - Use existing `Modal` component for confirmation dialog (I7)

5. **Accessibility (CRITICAL for W8)**:
    - Add `padding-bottom` to main content so sticky save bar doesn't obscure last field
    - Hamburger menu already handled in Task 3 (`aria-expanded`, focus trap)
    - **Add:** Respect `prefers-reduced-motion` for smooth scrolling (disable smooth scroll if user prefers reduced motion)
    - **Add:** "Skip to content" link for keyboard users (add at top of page)
    - **Add:** Mobile active-section indicator (already in Task 3)

6. **Important**: Load full Config struct, mutate only edited fields, serialize whole struct back. Do NOT rebuild TOML from partial data.

**Steps:**
- [ ] Write failing unit test for ConfigEditor in `crates/koji-web/src/pages/config_editor.rs` (in `#[cfg(test)] mod tests`)
  - Test: state management (config, pending_changes, save_status, active_section)
  - Test: API calls use correct endpoints (/api/config/structured)
  - Test: SaveStatus enum works correctly (including Validating state)
  - Test: per-section diff logic for restart banner
- [ ] Run `cargo test --package koji-web --features ssr --lib config_editor::tests`
  - Did it fail? If passed, investigate.
- [ ] Define enum SaveStatus { Idle, Validating, Saving, Success(String), Error(String) } in config_editor.rs
- [ ] Implement SectionContainer component with `description: AnyView`
- [ ] Implement SaveBar component
- [ ] Implement ConfigEditor page with state management
- [ ] **W10:** Optimize pending_changes to avoid full Config clones (use section-scoped signals or `update()`)
- [ ] Add GET /api/config/structured loading on mount
- [ ] Add POST /api/config/structured save on "Save All" click (raw JSON Config body)
- [ ] **W1:** Implement per-section diff for restart banner:
  - Derive `PartialEq` on General, BackendConfig, Supervisor, SamplingParams, ModelConfig
  - Compare loaded vs pending for non-proxy sections only
  - Show appropriate banner based on diff result
- [ ] Add Reload button with confirmation dialog (use existing Modal component)
- [ ] Add success/error message handling
- [ ] **W8:** Add accessibility enhancements:
  - Add `padding-bottom` to main content
  - Respect `prefers-reduced-motion` for scroll
  - Add "Skip to content" link
- [ ] Run `cargo test --package koji-web --features ssr --lib config_editor::tests`
  - Did all tests pass?
- [ ] Run `cargo fmt --all`
  - Did it succeed?
- [ ] Run `cargo check --package koji-web`
  - Did it succeed?
- [ ] Commit with message: "feat: implement main ConfigEditor page with structured API"

**Acceptance criteria:**
- [ ] ConfigEditor page renders navigation + 4 sections + save bar
- [ ] GET /api/config/structured loads config on mount
- [ ] POST /api/config/structured saves config on "Save All" click (raw JSON body)
- [ ] Full Config struct round-trip (preserves models, loaded_from manually restored)
- [ ] Save status includes Validating state
- [ ] Save All aborts on validation error, scrolls to first error, focuses it
- [ ] Per-section diff implemented: only non-proxy changes trigger restart banner
- [ ] Proxy-only changes show "hot-reloaded" message (no restart banner)
- [ ] Success message shown on save
- [ ] Error handling for 400 (deserialization) and 500 (server) responses
- [ ] Reload button discards changes with confirmation (uses Modal component)
- [ ] Sticky save bar doesn't obscure last field (padding-bottom)
- [ ] Respects prefers-reduced-motion for smooth scrolling
- [ ] Has "Skip to content" link
- [ ] All tests passing
- [ ] No clippy warnings

**CRITICAL FIXES FROM REVIEWER:**
- W1: Per-section diff for restart banner (derive PartialEq, compare non-proxy sections)
- W9: Save All partial-failure behavior (abort, scroll to first error, focus, aria-live)
- W8: Accessibility (padding-bottom, prefers-reduced-motion, skip-to-content)
- W10: Optimize pending_changes to avoid full Config clones
- I4: Add Validating state to SaveStatus
- I6: Use `AnyView` for description (allows inline links)
- I7: Use existing Modal component for confirmation dialog

---

## Task 10: Add Integration Tests and Documentation

**Context:**
Per AGENTS.md TDD requirement, we need comprehensive test coverage and documentation. This task adds integration tests and updates documentation.

**Critical note:** Round-trip test should have been written in Task 0. This task focuses on comprehensive integration tests and documentation.

**Files:**
- Create: `crates/koji-web/tests/config_integration_test.rs`
- Create: `docs/config-page-user-guide.md`
- Create: `docs/config-page-developer-guide.md`
- Modify: `crates/koji-web/README.md` (if exists)

**What to implement:**

1. **Integration tests** (in `config_integration_test.rs`):
    - Test: full config round-trip without losing fields (models, sampling_templates, loaded_from)
    - Test: POST /api/config/structured error handling (400, 500)
    - Test: sampling fields serialization (enabled=false = None, enabled=true = Some(value))
    - Test: built-in templates cannot be deleted via API
    - Test: proxy hot-reload via sync_proxy_config
    - Test: **Equivalence test** - POST via `/api/config` (TOML), GET via `/api/config/structured`, POST JSON back, compare disk content (W5)
    - Test: **BUILTIN_TEMPLATES sync** - verify hardcoded list matches `Profile::all()` (I5)
    - Note: Add `[[test]] name = "config_integration_test" path = "tests/config_integration_test.rs" required-features = ["ssr"]` to Cargo.toml
    - **I2:** Note: Inline `#[cfg(test)] mod tests` blocks are picked up automatically by `cargo test --lib`. Only `tests/*.rs` integration files need a `[[test]]` declaration.

2. **Documentation**:
    - User guide: how to use the config page, field descriptions, validation rules
    - Developer guide: architecture decisions, data flow, component relationships
    - API documentation: explain GET/POST /api/config/structured endpoints
    - Update spec: mark as "Approved" with implementation notes
    - Document C3 resolution: explain how health_check_timeout_ms and health_check_retries are handled

3. **Final verification**:
    - Run full test suite: `cargo test --workspace`
    - Run clippy: `cargo clippy --workspace -- -D warnings`
    - Run fmt: `cargo fmt --all`
    - Verify no regressions in existing features

**Steps:**
- [ ] Add `[[test]] name = "config_integration_test" path = "tests/config_integration_test.rs" required-features = ["ssr"]` to `crates/koji-web/Cargo.toml`
- [ ] Write integration test for full config round-trip (verify models, sampling_templates preserved; loaded_from manually restored)
- [ ] Write integration test for POST /api/config/structured error handling (400, 500)
- [ ] Write integration test for sampling fields serialization
- [ ] Write integration test for built-in template protection
- [ ] Write integration test for proxy hot-reload via sync_proxy_config
- [ ] Write equivalence test: /api/config (TOML) and /api/config/structured (JSON) produce same disk content
- [ ] Write BUILTIN_TEMPLATES sync test with `Profile::all()`
- [ ] Run `cargo test --package koji-web --test config_integration_test`
  - Did all tests pass?
- [ ] Create user guide documentation (`docs/config-page-user-guide.md`)
- [ ] Create developer guide documentation (`docs/config-page-developer-guide.md`)
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
  - Did it succeed? If not, fix warnings.
- [ ] Run `cargo fmt --all`
  - Did it succeed?
- [ ] Commit with message: "feat: add integration tests and documentation"

**Acceptance criteria:**
- [ ] Integration tests cover key scenarios (round-trip, errors, serialization)
- [ ] Equivalence test between /api/config and /api/config/structured passes
- [ ] BUILTIN_TEMPLATES sync test with `Profile::all()` passes
- [ ] User guide explains config page usage
- [ ] Developer guide documents architecture
- [ ] All tests passing (unit + integration)
- [ ] No clippy warnings
- [ ] No formatting issues
- [ ] No regressions in existing features

**CRITICAL FIXES FROM REVIEWER:**
- W5: Add equivalence test between /api/config (TOML) and /api/config/structured (JSON)
- I5: Add BUILTIN_TEMPLATES sync test with `Profile::all()`
- I2: Remove unnecessary `[[test]]` entries for inline #[cfg(test)] modules

---

## Implementation Order

Execute tasks in order: 0 → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 10

**Important notes:**
- Task 0 MUST be completed first (all other tasks depend on JSON API)
- **BLOCKER BEFORE ANY TASKS:** Resolve C3 (health_check_timeout_ms/retries) before starting Task 0
- Each task is independently commitable and follows TDD pattern
- Use `cargo test --package koji-web --features ssr --lib <module>::tests` for inline unit tests
- Use `cargo test --package koji-web --test <name>` for integration tests with [[test]] entry

**Task numbering:** Tasks are numbered 0-8, then 10 (Task 9 was absorbed into Task 1 for accessibility). No renumbering needed.

---

## Summary of Critical Fixes from Reviewer

### Must Fix Before Implementation (Critical)

| Fix | Description | Status |
|-----|-------------|--------|
| **C1** | Add `#[serde(flatten)] extra` field to all mirror types for forward compatibility | ✅ Updated in Task 0 |
| **C2** | Update docs: Axum returns 400 for all JSON errors (not 422) | ✅ Updated in Task 0 |
| **C3** | **BLOCKER:** Resolve health_check_timeout_ms/retries in Supervisor struct | ⚠️ **ACTION REQUIRED** - See Task 6 |
| **C4** | Reconcile top_k range: use 0-100 (matches llama.cpp) | ✅ Updated in Tasks 4, 7 |
| **C5** | Specify `cfg.save_to(&state.config_path.parent()?)` for persistence | ✅ Updated in Task 0 |

### Must Fix During Implementation (Warnings)

| Fix | Description | Task |
|-----|-------------|------|
| **W1** | Per-section diff for restart banner (derive PartialEq) | Task 8 |
| **W2** | Use `std::net::IpAddr::from_str`, `url::Url::parse` for validation | Task 1 |
| **W3** | Add wasm-bindgen-test smoke test for FormInput | Task 1 (optional) |
| **W4** | Populate help_text for all numeric fields with source citations | Tasks 1, 4 |
| **W5** | Add equivalence test between /api/config and /api/config/structured | Task 10 |
| **W6** | Add default helper functions for all mirror types | Task 0 |
| **W7** | Built-in template rename protection (read-only name field) | Task 7 |
| **W8** | Accessibility (focus trap, reduced motion, skip-to-content) | Tasks 3, 8 |
| **W9** | Save All partial-failure behavior (abort, scroll, focus) | Task 8 |
| **W10** | Optimize pending_changes to avoid full Config clones | Task 8 |
| **W11** | Explicit From<Option<f64>> for SamplingField loading rule | Task 2 |
| **W12** | Update default_args parsing to filter empty lines | Task 5 |

### Nice-to-Have Improvements (Info)

| Fix | Description |
|-----|-------------|
| I1 | Task numbering is correct as-is (0-8, 10) |
| I2 | Remove unnecessary [[test]] entries for inline #[cfg(test)] modules |
| I3 | Consider extracting debounce helper (optional) |
| I4 | Add Validating state to SaveStatus enum |
| I5 | Add BUILTIN_TEMPLATES sync test with Profile::all() |
| I6 | Use AnyView for SectionContainer description |
| I7 | Reuse existing Modal component (don't create new) |
| I8 | Consider URL state for active_section (optional) |
| I9 | Address empty config edge case in General section |

---

**Reviewers:** Please review and provide feedback on:
- Task breakdown and dependencies
- Test coverage adequacy
- Implementation approach
- Missing edge cases
- Accessibility considerations

**Go/No-Go Decision:** **APPROVED WITH FIXES** - All Critical issues addressed in plan. Blocker C3 must be resolved before implementation begins.
