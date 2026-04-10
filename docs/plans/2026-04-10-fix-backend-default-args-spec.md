# Fix Backend Default Args & Move Save Button — Spec

## Problem

Two issues in the backends webui:

1. **Bug: `default_args` never displayed.** The API handlers in `api/backends.rs` call `Config::load_from(&config_path)` with a **file path** (e.g., `~/.config/koji/config.toml`), but `Config::load_from()` expects a **directory path** (e.g., `~/.config/koji/`). It internally calls `fs::create_dir_all(config_dir)` and then joins `"config.toml"`, so passing a file path causes it to fail silently. The `default_args_map` ends up empty, and `skip_serializing_if = "Vec::is_empty"` on the DTOs causes the field to be omitted from the JSON response entirely. The same bug affects `Config::save_to()` in the update endpoint.

2. **UX: Per-card save buttons.** Each backend card has its own "Save" button for `default_args`, plus an `on:blur` save that fires independently. This is inconsistent with the config editor page which uses a single "Save Changes" button in the top right corner. There's also no save feedback (success/failure) in the current implementation — the response is discarded (`let _ = ...`).

## Approved Design

Follow the **config-editor pattern**: single page-level save button in the top right, all `default_args` edits accumulate locally, one bulk save action.

### API Fix (Bug)

Four call sites in `crates/koji-web/src/api/backends.rs` need fixing:

| Line | Function | Bug | Fix |
|------|----------|-----|-----|
| 444 | `list_backends` | `Config::load_from(&config_path)` | Change to `Config::load_from(&config_dir)` (variable already computed on line 422) |
| 1211 | `check_backend_updates` | `Config::load_from(&config_path)` | Change to `Config::load_from(&config_dir)` (variable already computed on line 1189) |
| 1503 | `update_backend_default_args` | `Config::load_from(&config_path)` | Compute `config_dir` from `config_path.parent()` and use it |
| 1534 | `update_backend_default_args` | `config.save_to(&config_path)` | Change to `config.save_to(&config_dir)` |

Reference pattern: `load_config_from_state()` in `api.rs` (lines 275-300).

### Frontend UX Change

**Remove** per-card save logic from `BackendCard`:
- Remove the `on:blur` save handler on the `default_args` input
- Remove the "Save" button from `BackendCard`

**Add** page-level save to `Backends` page:
- Add a `save_status: RwSignal<Option<String>>` signal
- Add a `default_args_edits: RwSignal<HashMap<String, String>>` signal to track dirty edits (backend_type → edited args string)
- Display an input for `default_args` inside each `BackendCard` but route changes through a callback that updates `default_args_edits`
- Add "Save Changes" button next to the `<h1>` in `.page-header`, styled identically to the config editor
- On save: POST to `/api/backends/{name}/default-args` for each edited backend, then refresh
- Show save status feedback ("Saving…", "✅ Saved", "❌ error") next to the button

### DTO Change

Remove `#[serde(skip_serializing_if = "Vec::is_empty")]` from `default_args` in both:
- `crates/koji-web/src/api/backends.rs` (line 48) — `BackendCardDto.default_args`
- `crates/koji-web/src/components/backend_card.rs` (line 83) — frontend `BackendCardDto.default_args`

This ensures `default_args` is always present in the JSON response, even when empty.