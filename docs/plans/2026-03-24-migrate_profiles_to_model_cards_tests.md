# [Feature] Test migrate_profiles_to_model_cards

**Goal:** Write failing tests for `migrate_profiles_to_model_cards` function
**Status:** DONE - Tests integrated into migration plan

**Architecture:** 
- Create a `Config` struct that wraps `profiles/` and `configs/` directories
- Create a `ModelCard` struct that wraps a model card file
- Implement `migrate_profiles_to_model_cards` to handle profile migration logic
- Write 8 test cases as specified in the task

**Tech Stack:** 
- `tempfile::tempdir()` for simulating file system
- `toml::from_str` for parsing TOML config files
- `anyhow::Result` for error handling

---

### Task 1: Test (profiles, modified): given `profiles/coding.toml` with modified values and a model card in `configs/` with no `[sampling.coding]`, migration inserts the profile into the card

**Files:**
- Create: `crates/kronk-core/src/config/migrate.rs` (test code)

**Steps:**
- [ ] Create test helper function to setup test environment
- [ ] Write test that creates `profiles/coding.toml` with modified values
- [ ] Create model card in `configs/` without `[sampling.coding]`
- [ ] Call `migrate_profiles_to_model_cards`
- [ ] Verify profile was inserted into card
- [ ] Run test, verify it fails

---

### Task 2: Test (profiles, card has it): same scenario but model card already has `[sampling.coding]` — migration skips (card wins)

**Files:**
- Modify: `crates/kronk-core/src/config/migrate.rs`

**Steps:**
- [ ] Write test that creates `profiles/coding.toml` with modified values
- [ ] Create model card in `configs/` with existing `[sampling.coding]`
- [ ] Call `migrate_profiles_to_model_cards`
- [ ] Verify migration was skipped (no changes to card)
- [ ] Run test, verify it fails

---

### Task 3: Test (profiles, already present): `coding.toml` exists but card already has `[sampling.coding]` — migration skips entirely

**Files**:
- Modify: `crates/kronk-core/src/config/migrate.rs`

**Steps**:
- [ ] Write test that creates `profiles/coding.toml` with explicit content
- [ ] Create model card in `configs/` with existing `[sampling.coding]`
- [ ] Call `migrate_profiles_to_model_cards`
- [ ] Verify migration was skipped (no changes to card)
- [ ] Run test, verify it fails

---

### Task 4: Test (profiles, non-built-in): `mypreset.toml` gets inserted into each card under `"mypreset"` unless already present

**Files:**
- Modify: `crates/kronk-core/src/config/migrate.rs`

**Steps:**
- [ ] Write test that creates `profiles/mypreset.toml` with custom values
- [ ] Create model card in `configs/` without `[sampling.mypreset]`
- [ ] Call `migrate_profiles_to_model_cards`
- [ ] Verify `mypreset` profile was inserted into card
- [ ] Run test, verify it fails

---

### Task 5: Test (profiles cleanup): after migration, `profiles/` directory is deleted

**Files:**
- Modify: `crates/kronk-core/src/config/migrate.rs`

**Steps:**
- [ ] Write test that creates `profiles/coding.toml` with modified values
- [ ] Create model card in `configs/` without `[sampling.coding]`
- [ ] Call `migrate_profiles_to_model_cards`
- [ ] Verify `profiles/` directory was deleted after migration
- [ ] Run test, verify it fails

---

### Task 6: Test (custom_profiles): given `config.custom_profiles` with `"fast"` entry, migration inserts it into each card's sampling under `"fast"` (unless already present), then sets `custom_profiles` to None

**Files:**
- Modify: `crates/kronk-core/src/config/migrate.rs`

**Steps:**
- [ ] Write test that creates `config.custom_profiles` with `"fast"` entry
- [ ] Create model card in `configs/` without `[sampling.fast]`
- [ ] Call `migrate_profiles_to_model_cards`
- [ ] Verify `fast` profile was inserted into card
- [ ] Verify `custom_profiles` was set to None
- [ ] Run test, verify it fails

---

### Task 7: Test (custom_profiles, card has it): card already has `[sampling.fast]` — migration skips

**Files:**
- Modify: `crates/kronk-core/src/config/migrate.rs`

**Steps:**
- [ ] Write test that creates `config.custom_profiles` with `"fast"` entry
- [ ] Create model card in `configs/` with existing `[sampling.fast]`
- [ ] Call `migrate_profiles_to_model_cards`
- [ ] Verify migration was skipped (no changes to card)
- [ ] Run test, verify it fails

---

### Task 8: Run tests and verify they fail

**Files:**
- Modify: `crates/kronk-core/src/config/migrate.rs`

**Steps:**
- [ ] Run `cargo test --package kronk-core migrate_profiles_to_model_cards`
- [ ] Verify all 8 tests fail as expected
- [ ] Commit work with descriptive message
