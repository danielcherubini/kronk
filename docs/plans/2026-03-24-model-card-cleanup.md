# [Cleanup] Re-exports and Model Card Documentation

**Goal:** Clean up unnecessary re-exports in CLI lib.rs and update documentation to reflect the new model card-based approach.
**Status:** ✅ COMPLETED - Part of unified model config (commit `95c8e01`)

**Architecture:** 
- Remove redundant public exports that expose internal implementation details
- Update model card documentation to clarify the new configs/ storage location
- Ensure consistency between code exports and documented functionality

**Tech Stack:**
- Rust CLI with clap for argument parsing
- TOML-based model card configuration

---

## Task 1: Clean up re-exports in CLI lib.rs

**Files:**
- Modify: `crates/koji-cli/src/lib.rs`

**Steps:**
- [ ] Review current re-exports: `handlers::server::{cmd_server_add, cmd_server_edit}`, `handlers::ExtractedFlags`, `flags::extract_koji_flags`
- [ ] Remove unnecessary public exports that expose internal implementation
- [ ] Keep only essential public API exports
- [ ] Add module-level documentation to explain what's exported

---

## Task 2: Update model card documentation in core lib.rs

**Files:**
- Modify: `crates/koji-core/src/lib.rs`

**Steps:**
- [ ] Add module-level documentation explaining model card-based approach
- [ ] Document that model cards are stored in `~/.config/koji/configs/<company>--<model>.toml`
- [ ] Explain that model cards contain quant info, context settings, and sampling presets
- [ ] Document auto-discovery of model cards from installed models

---

## Task 3: Update CLI lib.rs documentation for model card approach

**Files:**
- Modify: `crates/koji-cli/src/lib.rs`

**Steps:**
- [ ] Update the module-level documentation to mention model cards
- [ ] Clarify how models are discovered and configured
- [ ] Ensure documentation matches the new configs/ approach

---

## Verification

- Run: `cargo check --workspace`
- Run: `cargo test --workspace`
- Verify no breaking changes to public API

---

## Review Criteria

- Re-exports are minimal and intentional
- Documentation accurately reflects model card approach
- No breaking changes to public API
- Code follows existing conventions