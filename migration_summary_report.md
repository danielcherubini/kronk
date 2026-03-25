# Migration Summary Report: profiles.d to Model Cards

## Overview

This report documents the migration from `profiles.d/` directory structure to inline model cards in `configs.d/`.

## Timeline

**Commit**: `9154b57` (Tue Mar 24 21:06:26 2026 +0100)
**Title**: "feat: implement profiles.d migration to model cards"

## Key Changes

### 1. Core Functionality Added

#### `migrate_profiles_to_model_cards()` Function
**File**: `crates/kronk-core/src/config/migrate.rs` (108 lines added)

The new function performs the following operations:

1. **Profile Collection**: 
   - Collects profiles from `profiles.d/` directory
   - Collects profiles from `config.custom_profiles` (always custom)
   - Builds a unified list of all profiles to migrate

2. **Model Card Enhancement**:
   - Loads all model cards from `configs.d/`
   - For each profile, inserts sampling configuration into model cards
   - Only inserts if key doesn't already exist (card takes precedence)

3. **Cleanup**:
   - Removes processed `.toml` files from `profiles.d/`
   - Removes empty `profiles.d/` directory if no remaining files
   - Sets `custom_profiles = None` and saves config

### 2. Sampling Templates Built-in

**File**: `crates/kronk-core/src/config/types.rs` (added to `Config` struct)

- Added `sampling_templates` field to `Config` struct
- Populated with built-in profile values from `Profile::all()`
- Enables default sampling behavior for all profiles

### 3. Config API Updates

#### `configs_dir()` method removed
**File**: `crates/kronk-core/src/config/loader.rs`

- Replaced with direct access to `general.models_dir` field
- Simplified directory resolution logic

#### `models_dir()` method removed
**File**: `crates/kronk-core/src/config/loader.rs`

- Replaced with direct access to `general.models_dir` field
- Simplified directory resolution logic

### 4. Default Config Updates

#### `sampling_templates` field added
**File**: `crates/kronk-core/src/config/types.rs`

- Added `sampling_templates: HashMap<String, HashMap<String, Value>>` field
- Populated in `Config::default()` with built-in profile parameters

#### `custom_profiles` field changed
**File**: `crates/kronk-core/src/config/types.rs`

- Changed from `HashMap<String, Sampling>` to `Option<HashMap<String, Sampling>>`
- Allows tracking of custom profiles during migration

## Files Modified

| File | Lines Changed | Purpose |
|------|---------------|---------|
| `crates/kronk-core/src/config/loader.rs` | -25 | Remove `configs_dir()` and `models_dir()` methods |
| `crates/kronk-core/src/config/migrate.rs` | +108 | Implement migration function |
| `crates/kronk-core/src/config/mod.rs` | +1, -1 | Export new `migrate_profiles_to_model_cards` function |
| `crates/kronk-core/src/config/resolve.rs` | +14, -8 | Update model card resolution to use new API |
| `crates/kronk-core/src/config/types.rs` | +23, -7 | Add `sampling_templates` and update `custom_profiles` type |
| `docs/plans/2026-03-24-migrate_profiles_to_model_cards_tests.md` | +126 | Test plan documentation |

## Test Coverage

Comprehensive test plan created with 8 test cases covering:

1. **Profile Migration**:
   - Modified profiles get inserted into cards
   - Existing card values take precedence
   - Unmodified profiles are skipped

2. **Custom Profiles**:
   - `config.custom_profiles` entries get migrated
   - Existing card values take precedence
   - `custom_profiles` set to `None` after migration

3. **Cleanup**:
   - `profiles.d/` directory deleted after migration
   - Empty directories removed

## Impact

### Breaking Changes
- `Config::configs_dir()` method removed - use `config.configs_dir()` instead
- `Config::models_dir()` method removed - use `config.general.models_dir` instead
- `Config.custom_profiles` type changed from `HashMap` to `Option<HashMap>`

### New Functionality
- `migrate_profiles_to_model_cards()` function available for programmatic migration
- Built-in sampling templates for all profiles
- Automatic cleanup of `profiles.d/` directory

## Migration Verification

The migration can be verified by:

1. Running existing tests to ensure no regressions
2. Checking that `profiles.d/` directory is removed after migration
3. Verifying model cards contain sampling configurations
4. Confirming `custom_profiles` is set to `None` after migration

## Conclusion

The migration successfully consolidates profile management from a separate directory structure into individual model cards, simplifying the codebase and improving maintainability while preserving existing functionality.