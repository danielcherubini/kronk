# Config Page Redesign Specification

**Date:** 2026-04-07  
**Author:** Daniel Cherubini  
**Status:** Draft - Awaiting Review

## Overview

This document specifies the redesign of the config editor page in the Koji web UI. The current implementation uses a raw TOML text editor, which is not user-friendly for non-technical users. The new design will provide a structured, form-based interface with grouped settings.

## Goals

- Provide a user-friendly, form-based config editor
- Group related settings into logical sections
- Support inline validation with debounced feedback
- Enable easy editing of complex nested structures (backends, sampling templates)
- Maintain compatibility with existing TOML config file format

## Non-Goals

- Adding new config fields (this is purely a UI redesign)
- Backend API changes (use existing `/api/config` endpoints)
- Real-time config reloading (changes require service restart)
- Migration of existing config files

## User Interface Design

### Layout Structure

```
┌─────────────────────────────────────────────────────────────┐
│  Koji Web UI                          [User Menu]           │
├──────────────┬──────────────────────────────────────────────┤
│              │                                               │
│  Navigation  │              Content Area                     │
│              │                                               │
│  • General   │  [Section Title]                              │
│  • Backends  │  [Section Description]                        │
│  • Supervisor│  [Form Fields]                                │
│  • Sampling  │                                               │
│              │  [Save Button]                                │
│              │                                               │
│              │  [Status Messages]                            │
│              │                                               │
├──────────────┴──────────────────────────────────────────────┤
│  [Save All] [Reload]                                         │
└─────────────────────────────────────────────────────────────┘
```

### Navigation (Left Sidebar)

- **Always visible on desktop**, hidden behind hamburger menu on mobile
- Shows 4 sections: General, Backends, Supervisor, Sampling
- Active section is highlighted
- Clicking navigation item scrolls to that section
- Collapsible to save space on smaller screens

### Section Order

1. **General** - Application-wide settings + Proxy configuration
2. **Backends** - Backend server configurations
3. **Supervisor** - Process management settings
4. **Sampling Templates** - LLM sampling parameter presets

---

## Section Specifications

### 1. General Section (with Proxy)

**Purpose:** Core application settings and proxy configuration merged into one section.

**Fields:**

| Field | Type | Default | Validation | Help Text Source | Description |
|-------|------|---------|------------|------------------|-------------|
| Log Level | Dropdown | info | Must be one of: debug, info, warn, error | N/A | Application logging verbosity |
| Models Directory | Text input | (empty) | Valid directory path format | N/A | Path to models directory (optional, uses default if empty) |
| Logs Directory | Text input | (empty) | Valid directory path format | N/A | Path to logs directory (optional, uses default if empty) |
| **Proxy Settings** | **Header** | | | **Visual separator** | |
| Proxy Enabled | Checkbox | false | None | N/A | Enable/disable proxy server |
| Host | Text input | 0.0.0.0 | `std::net::IpAddr::from_str` | N/A | Proxy server bind address |
| Port | Number input | 11434 | 1-65535 | Valid TCP port range (IANA) | Proxy server port |
| Idle Timeout (secs) | Number input | 300 | 1-3600 | llama.cpp default: 300s | Connection idle timeout |
| Startup Timeout (secs) | Number input | 120 | 1-600 | Backend startup reasonable limit | Max time to wait for backend startup |
| Circuit Breaker Threshold | Number input | 3 | 1-100 | Reasonable failure count before circuit opens | Failures before circuit opens |
| Circuit Breaker Cooldown (secs) | Number input | 60 | 1-300 | 60s default in spec | Time before circuit resets |
| Metrics Retention (secs) | Number input | 86400 | 60-604800 | 1 day to 1 week range | How long to keep metrics data |

**UI Notes:**
- Proxy settings section visually separated with a horizontal rule or header
- All fields use debounced validation (500ms after typing stops)
- Inline error messages appear below each field
- Success state shown with green border/checkmark

---

### 2. Backends Section

**Purpose:** Configure backend server paths and default arguments.

**UI Pattern:** Editable list/table of backends

**Structure:**

```
┌──────────────────────────────────────────────────────────┐
│ Backends Configuration                                   │
│ Configure paths and default arguments for backend        │
│ servers (llama.cpp, ollama, etc.)                        │
│                                                          │
│ ┌────────────────────────────────────────────────────┐  │
│ │ llama.cpp (default)                      [Edit ▼]  │  │
│ ├────────────────────────────────────────────────────┤  │
│ │ Path:           [/usr/local/bin/llama-server]      │  │
│ │ Default Args:   [--port 8080, --threads 4]         │  │
│ │ Health Check:   [http://localhost:8080/health]     │  │
│ │ Version:        [v0.2.0]                           │  │
│ └────────────────────────────────────────────────────┘  │
│                                                          │
│ ┌────────────────────────────────────────────────────┐  │
│ │ ollama                                 [Edit ▼]    │  │
│ ├────────────────────────────────────────────────────┤  │
│ │ Path:           [/usr/bin/ollama]                  │  │
│ │ Default Args:   []                                 │  │
│ │ Health Check:   [http://localhost:11434/api/health]│  │
│ │ Version:        [0.1.35]                           │  │
│ └────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

**Fields per Backend:**

| Field | Type | Validation | Notes |
|-------|------|------------|-------|
| Path | Text input | File exists check (if provided) | Full path to backend binary |
| Default Args | Text input | None | Space or comma-separated args |
| Health Check URL | Text input | Valid URL format | Optional health check endpoint |
| Version | Text input | None | Version string for pinning |

**UI Notes:**
- Each backend shown as a card/row
- Edit button expands/collapses fields
- Inline validation on blur
- No add/remove functionality (planned for later)
- Shows count of configured backends

---

### 3. Supervisor Section

**Purpose:** Configure process supervision and restart behavior.

**Fields:**

| Field | Type | Default | Validation | Description |
|-------|------|---------|------------|-------------|
| Restart Policy | Dropdown | always | Must be: always, on-failure, never | When to restart crashed processes |
| Max Restarts | Number input | 10 | 0-1000 | Maximum restart attempts before giving up |
| Restart Delay (ms) | Number input | 3000 | 100-60000 | Wait time between restart attempts |
| Health Check Interval (ms) | Number input | 5000 | 1000-60000 | How often to check process health |

**UI Notes:**
- Simple form layout
- All fields with inline validation
- Help text shown below each field
- Debounced validation (500ms)

---

### 4. Sampling Templates Section

**Purpose:** Manage LLM sampling parameter presets.

**UI Pattern:** Card-based grid with add/remove functionality

**Structure:**

```
┌──────────────────────────────────────────────────────────┐
│ Sampling Templates                                       │
│ Configure temperature, top_p, and other sampling         │
│ parameters for LLM inference. Each template represents   │
│ a preset configuration.                                  │
│                                                          │
│ ┌─ Add Template ─────────────────────────────────────┐  │
│                                                          │
│ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐   │
│ │ Coding       │ │ Chat         │ │ Analysis     │   │
│ │ temp: 0.3    │ │ temp: 0.7    │ │ temp: 0.3    │   │
│ │ top_p: 0.9   │ │ top_p: 0.95  │ │ top_p: 0.9   │   │
│ │ [Edit] [×]   │ │ [Edit] [×]   │ │ [Edit] [×]   │   │
│ └──────────────┘ └──────────────┘ └──────────────┘   │
│                                                          │
│ ┌──────────────┐                                         │
│ │ Creative     │                                         │
│ │ temp: 0.9    │                                         │
│ │ top_p: 0.95  │                                         │
│ │ [Edit] [×]   │                                         │
│ └──────────────┘                                         │
└──────────────────────────────────────────────────────────┘
```

**Template Fields (when editing):**

| Field | Type | Default | Validation | Help Text Source | Description |
|-------|------|---------|------------|------------------|-------------|
| Name | Text input | (auto-generated) | Required, unique | N/A | Template identifier |
| Temperature | Number input | 1.0 | 0.0-2.0 | llama.cpp temperature range | Response randomness |
| Top K | Number input | 40 | 0-100 | 0 disables top-k filtering in llama.cpp | Consider top K tokens |
| Top P | Number input | 1.0 | 0.0-1.0 | llama.cpp top_p range | Cumulative probability |
| Min P | Number input | 0.0 | 0.0-1.0 | llama.cpp min_p range | Minimum probability threshold |
| Presence Penalty | Number input | 0.0 | -2.0-2.0 | llama.cpp penalty range | Penalize repeated topics |
| Frequency Penalty | Number input | 0.0 | -2.0-2.0 | llama.cpp penalty range | Penalize repeated tokens |
| Repeat Penalty | Number input | 1.1 | 0.5-2.0 | llama.cpp --repeat-penalty range | Penalize repeated sequences |

**Note on Top K:** Range is 0-100 (not 1-100 as previously stated). Value of 0 disables top-k filtering in llama.cpp.

**UI Notes:**
- Grid layout for template cards (responsive: 1 col mobile, 2 col tablet, 3-4 col desktop)
- Each card shows template name and key stats (temperature, top_p)
- Edit button opens inline editor or modal
- Delete button (×) removes template with confirmation
- "Add Template" button opens creation form
- Built-in templates (coding, chat, analysis, creative) cannot be deleted
- Custom templates can be added/removed
- Inline validation on all fields
- Debounced validation (500ms)

---

## Interaction Design

### Navigation

- Clicking navigation item scrolls smoothly to that section
- Active section highlighted in sidebar
- On mobile, hamburger menu toggles sidebar visibility
- Sidebar can be collapsed to show only icons (optional enhancement)

### Form Validation

**Trigger:** Debounced validation (500ms after typing stops)

**Validation States:**
- **Default:** Gray border
- **Valid:** Green border, checkmark icon
- **Invalid:** Red border, error message below field
- **Pending:** Loading spinner (while debounce timer runs)

**Error Messages:**
- Shown directly below the field
- Clear, actionable text (e.g., "Port must be between 1 and 65535")
- Dismissed when field is corrected

### Save Behavior

**Save All Button:**
- Fixed at bottom of page (sticky footer)
- Validates all sections before saving
- Shows progress indicator during save
- Displays success/error message after save
- Disabled while save is in progress

**Save Flow:**
1. User clicks "Save All"
2. All sections validated (triggers validation if not already valid)
3. If any errors, scroll to first error and highlight
4. If all valid, send PUT request to `/api/config`
5. Show loading spinner
6. On success: green success message, auto-hide after 3 seconds
7. On error: red error message with details

---

## Technical Requirements

### Components Needed

1. **SideNavigation** - Left sidebar with section links
2. **SectionContainer** - Wrapper for each config section
3. **FormInput** - Reusable text/number input with validation
4. **FormSelect** - Dropdown select with validation
5. **FormCheckbox** - Checkbox with label
6. **BackendCard** - Editable backend configuration card
7. **SamplingTemplateCard** - Grid card for sampling templates
8. **SamplingTemplateEditor** - Form for editing/creating templates
9. **StatusMessage** - Success/error message display
10. **SaveBar** - Fixed bottom bar with Save All button

### State Management

- Use Leptos signals for reactive state
- Store form values in `RwSignal`
- Track validation errors in separate signal
- Track overall save state (idle, saving, success, error)

### API Integration

**GET /api/config**
- Returns full config as TOML string
- Parse TOML into structured data for form population

**PUT /api/config**
- Accepts full config as TOML string
- Returns success/failure status
- Handle 400 errors (validation) and 500 errors (server errors)

### Data Flow

```
1. Page loads → GET /api/config
2. Parse TOML → Populate form state
3. User edits → Update signals, trigger debounce validation
4. Validation errors → Show inline messages
5. User clicks Save All → Validate all, then PUT /api/config
6. Response → Show status message
7. On success → Reload config or show "Saved!" message
```

### Styling

- Reuse existing form components from dashboard.rs, model_editor.rs
- Use existing CSS classes: `.form-group`, `.form-card`, `.btn-primary`, etc.
- Add new classes for config-specific layouts: `.config-sidebar`, `.config-section`, `.template-grid`
- Maintain dark theme consistency
- Responsive design for mobile/tablet

---

## Edge Cases & Error Handling

### Empty Config

- Show empty state message: "No configuration found. Start by setting log level."
- Allow saving with minimal required fields

### Invalid TOML

- Show error: "Failed to parse config file. Please check syntax."
- Keep editor in error state, don't overwrite

### Backend Path Not Found

- Show warning: "Backend path not found: {path}"
- Allow saving but mark as warning (not blocking)

### Duplicate Template Names

- Show error: "Template name already exists"
- Prevent save until unique

### Network Error

- Show error: "Failed to save config. Please try again."
- Keep form state intact

---

## Future Enhancements (Out of Scope)

- Add new backends directly from UI
- Upgrade backends from UI
- Import/export config files
- Config diff viewer
- Config version history
- Per-model config overrides
- Real-time config preview
- Config validation against schema

---

## Success Criteria

- [ ] All config sections accessible via navigation
- [ ] Inline validation working on all fields
- [ ] Save All button validates and saves all sections
- [ ] Sampling templates can be added/edited/removed
- [ ] Backends can be edited
- [ ] Responsive design works on mobile
- [ ] Dark theme maintained
- [ ] No breaking changes to existing config files
- [ ] All tests passing

---

## Next Steps

1. Review and approve this spec
2. Create implementation plan with tasks
3. Implement side navigation component
4. Implement each section form
5. Add validation logic
6. Integrate with API
7. Test all edge cases
8. Deploy and verify

---

**Reviewers:** Please review and provide feedback on:
- Completeness of field specifications
- UI/UX decisions
- Technical feasibility
- Missing edge cases
- Accessibility considerations

---

## Critical Fixes from Review (Must Address Before Implementation)

### 1. API Contract Correction
- **HTTP Method:** Use `POST /api/config/structured` (not PUT, not `/api/config`)
- **Request Body:** Raw JSON `Config` struct (not wrapped in object, not TOML string)
- **Error Codes:** 
  - 400 Bad Request: JSON deserialization error (Axum's `Json<T>` extractor returns 400 for ALL JSON errors, both syntactic and semantic - there is no 422)
  - 500 Internal Server Error: Server-side error (file write failure, etc.)
  - 404 Not Found: Config path not configured
- **Implementation:** Create new endpoint `/api/config/structured` parallel to existing `/api/config` (TOML)
- **Persistence:** Use `cfg.save_to(&state.config_path.parent()?)` for consistency with existing endpoints
- **loaded_from:** Restore from existing proxy config before persisting (it has `#[serde(skip)]`)
- **Hot-reload:** Call `sync_proxy_config` after successful save for proxy settings
- **Forward-compat:** Mirror types must include `#[serde(flatten, skip_serializing_if = "Option::is_none")] pub extra: Option<serde_json::Map<String, serde_json::Value>>` to prevent data loss from future upstream changes

### 2. Sampling Params Pattern
- **Problem:** All `SamplingParams` fields are `Option<T>` with `skip_serializing_if`
- **Impact:** Setting defaults (1.0, 40, etc.) would emit CLI args that previously weren't sent
- **Solution:** Reuse pattern from `model_editor.rs:67-71`:
  ```rust
  pub struct SamplingField {
      pub enabled: bool,
      pub value: String,
  }
  ```
- **UI:** Each field has an "Enabled" toggle; only enabled fields are serialized

### 3. Supervisor Field Audit (BLOCKER - Must Resolve Before Implementation)
- **Issue:** `health_check_timeout_ms = 30000` and `health_check_retries = 3` exist in `config/koji.toml` (lines 19-20) but are NOT in `Supervisor` struct in `koji-core/src/config/types.rs`
- **Risk:** These fields will be **silently dropped** on first save through the new UI, causing data loss in existing user configs
- **Action Required (Choose One):**
  - **Option A (Preferred):** Add `health_check_timeout_ms: u64` and `health_check_retries: u32` to `Supervisor` struct in `koji-core/src/config/types.rs`
  - **Option B:** Remove these two lines from `config/koji.toml` example config to avoid confusion (but note: existing user configs with these fields will still lose them on save)
- **Also:** `restart_policy` has no usages outside `types.rs` - either wire it up to actual supervisor logic OR remove from UI and add help text "Not currently used by supervisor"
- **This is a BLOCKER:** Do not ship the config page until C3 is resolved

### 4. Full Config Round-Trip
- **Critical:** Must load entire `Config` struct (including `models` HashMap)
- **Risk:** Rebuilding TOML from form state would wipe model configurations
- **Solution:** Parse into full struct, mutate only edited fields, serialize whole struct
- **Preserve:** `Config.loaded_from` field (already handled in `api.rs:108-115`)

### 5. Built-in Template Protection
- **Decision:** UI-only enforcement (server has no validation)
- **Risk:** Users can delete built-ins via direct file edit or API calls
- **Mitigation:** Document this limitation; consider server-side merge to re-inject missing built-ins

### 6. Proxy Hot-Reload Contradiction
- **Non-Goal says:** "Real-time config reloading (changes require service restart)"
- **Reality:** `api.rs:76-80` calls `sync_proxy_config` which hot-reloads proxy settings
- **Resolution:** Update spec to clarify:
  - Proxy settings: hot-reload via `sync_proxy_config`
  - Other settings: require service restart
  - Show appropriate banner after save

### 7. Backend Args Tokenization
- **Problem:** `default_args` is `Vec<String>`, not comma/space-separated
- **Risk:** Args like `--system-prompt "you are helpful"` break on naive split
- **Solution:** 
  - **One arg per row in UI** (newline-separated textarea)
  - Parse on save: `text.lines().map(|s| s.trim()).filter(|s| !s.is_empty()).collect()`
  - **Document limitation in UI:** "Enter one argument per line. Each line becomes a separate CLI token. For `--flag value`, use two lines."
  - This is the simplest approach and avoids shlex complexity

### 8. Validation Limitations
- **Cannot do client-side:** File existence checks, directory validation
- **Solution:** Client-side validation is **syntactic-only** (format, range, URL validity)
- **Server-side:** File existence and directory validation can be added later if needed
- **Recommendation:** Implement syntactic checks client-side using:
  - `std::net::IpAddr::from_str` for IP addresses (handles v4 + v6)
  - `url::Url::parse` for URLs (add `url` crate to dependencies)
  - Range checks via plain comparison
- **Defer to server:** File/directory existence checks can be added via `POST /api/validate` endpoint later if needed

### 9. Accessibility Requirements
- **All inputs:** Proper `<label for=>` or `aria-labelledby`
- **Error messages:** `aria-describedby` + `role="alert"` or `aria-live="polite"`
- **Hamburger menu:** `aria-expanded` attribute + focus trap when open
- **Validation states:** Must use **icons + text**, not just color (WCAG 1.4.1)
  - Include checkmark (✓) for valid, error icon (✗) for invalid
- **Sticky save bar:** Add `padding-bottom` to main content so it doesn't obscure last field
- **Motion:** Respect `prefers-reduced-motion` for smooth scrolling
- **Keyboard users:** Add "Skip to content" link at top of page
- **Mobile nav:** Show active section indicator even when sidebar is closed
- **Save All validation errors:** Abort save, scroll to first error, focus it, announce via `aria-live`

### 10. Mobile Navigation
- **Breakpoint:** Define explicit breakpoint (e.g., `@media (max-width: 768px)`)
- **Hamburger menu:** Toggle button with `aria-expanded` attribute
- **Focus trap:** When menu is open, focus must not escape until menu closed
- **Close behavior:** Close on outside click
- **Active indicator:** Show active section indicator even when sidebar is closed (e.g., in header or as visual cue)

### 11. Template Rename Semantics
- **Risk:** Renaming template breaks model references (see `card.rs:88`)
- **Built-in templates:** Rename **disabled** (name field is read-only in editor)
- **Custom templates:** Rename allowed, but show confirmation: "Models referencing this template will break."
- **Name conflicts:** Show error "Template with this name already exists"
- **Built-in list:** Hardcode `["coding", "chat", "analysis", "creative"]` client-side, sync with `Profile::all()` via test

### 12. Numeric Range Justifications
- **Each range must be justified with source** (llama.cpp docs, backend defaults, IANA, etc.)
- **Add `help_text` field to each form field** with justification
- **Every numeric field must have populated help_text** (not just the prop)
- **Examples:**
  - Port: 1-65535 (Valid TCP port range, IANA)
  - Top K: 0-100 (0 disables top-k filtering in llama.cpp)
  - Temperature: 0.0-2.0 (llama.cpp temperature range)
  - Repeat Penalty: 0.5-2.0 (llama.cpp --repeat-penalty range)
  - Idle Timeout: 1-3600 (llama.cpp default: 300s)
- **Add acceptance criterion:** "Every numeric field has non-empty help_text citing source"

### 13. Test Coverage (TDD Requirement)
Per `AGENTS.md`, add testing section:
- Unit tests for form ↔ Config conversion
- Validation rule tests
- Smoke test: round-trip default config without losing fields
- Integration test: POST /api/config with various payloads

---

## Updated Success Criteria

- [ ] All config sections accessible via navigation
- [ ] Inline validation working on all fields (debounced, with proper error messages)
- [ ] Save All button validates and saves all sections via POST /api/config
- [ ] Sampling templates use enabled+value pattern to avoid emitting unnecessary fields
- [ ] Sampling templates can be added/edited/removed (built-ins cannot be deleted)
- [ ] Backends can be edited (no add/remove for now)
- [ ] Full config round-trip preserves models and other sections
- [ ] Responsive design works on mobile
- [ ] Dark theme maintained
- [ ] No breaking changes to existing config files
- [ ] All tests passing (unit + integration)
- [ ] Accessibility requirements met (ARIA labels, focus management, reduced motion)
- [ ] Proxy hot-reload documented and working
- [ ] Sticky banner shows appropriate message after save

---

## Implementation Priority

**Phase 1 - Core Structure:**
1. Side navigation component
2. Section containers with proper layout
3. Basic form inputs (text, number, select, checkbox)

**Phase 2 - Form Logic:**
4. Sampling params enabled+value pattern
5. Debounced validation logic
6. Full Config struct round-trip

**Phase 3 - Sections:**
7. General section (with proxy)
8. Backends section
9. Supervisor section
10. Sampling templates section (with add/remove)

**Phase 4 - Integration:**
11. POST /api/config integration
12. Error handling (422, 500)
13. Save bar with sticky banner
14. Reload button with confirmation

**Phase 5 - Polish:**
15. Accessibility enhancements
16. Mobile responsive fixes
17. Test coverage
18. Documentation updates

