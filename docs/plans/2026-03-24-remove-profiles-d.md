# Remove profiles.d — Model Cards Are the Single Source of Sampling Truth

**Goal:** Eliminate `profiles.d/`, `custom_profiles`, `Profile::Custom`, and
hardcoded built-in profile defaults. Model cards become the sole source of
sampling parameters. A new `[sampling_templates]` section in `config.toml`
provides seed values for new model cards created during `kronk model pull`.

**Architecture:** The `Profile` enum (coding, chat, analysis, creative)
becomes a pure label — a key into the model card's `[sampling.<profile>]`
section. It no longer carries hardcoded default params. The resolution
chain simplifies to: `modelcard [sampling.<profile>] -> server-level overrides`.
When `kronk model pull` creates a new card without a community card, it
copies the `[sampling_templates.*]` from config.toml into the card. A
one-time migration moves any user-customised profiles.d and custom_profiles
entries into existing model cards before cleaning up.

**Tech Stack:** Rust, TOML (serde), existing `kronk-core` / `kronk-cli` crates

---

### Task 1: Add `[sampling_templates]` to Config

**Files:**
- Modify: `crates/kronk-core/src/config/types.rs`
- Modify: `crates/kronk-core/src/config/loader.rs` (Default impl)
- Test: inline tests

**Steps:**
- [ ] Write test: default Config has `sampling_templates` with entries for
  all 4 built-in profiles (coding, chat, analysis, creative) populated
  with the values currently in `Profile::params()`
- [ ] Write test: `sampling_templates` round-trips through TOML serde
- [ ] Run tests, verify they fail
- [ ] Add `sampling_templates: HashMap<String, SamplingParams>` field to
  `Config` in `types.rs` (with `#[serde(default)]`)
- [ ] Populate it in `Config::default()` in `loader.rs` with the 4 built-in
  profile values (same values currently in `Profile::params()`)
- [ ] Run tests, verify they pass
- [ ] Commit

---

### Task 2: Auto-migrate profiles.d and custom_profiles into model cards

**Files:**
- Modify: `crates/kronk-core/src/config/migrate.rs`
- Test: `crates/kronk-core/src/config/migrate.rs` (inline tests)

**Steps:**
- [ ] Write test (profiles.d, modified): given `profiles.d/coding.toml` with
  modified values and a model card in `configs.d/` with no
  `[sampling.coding]`, migration inserts the profile into the card
- [ ] Write test (profiles.d, card has it): same scenario but model card
  already has `[sampling.coding]` — migration skips (card wins)
- [ ] Write test (profiles.d, unmodified): `coding.toml` matches
  `Profile::Coding.params()` — migration skips entirely
- [ ] Write test (profiles.d, non-built-in): `mypreset.toml` gets inserted
  into each card under `"mypreset"` unless already present
- [ ] Write test (profiles.d cleanup): after migration, `profiles.d/`
  directory is deleted
- [ ] Write test (custom_profiles): given `config.custom_profiles` with
  `"fast"` entry, migration inserts it into each card's sampling under
  `"fast"` (unless already present), then sets `custom_profiles` to None
- [ ] Write test (custom_profiles, card has it): card already has
  `[sampling.fast]` — migration skips
- [ ] Run tests, verify they fail
- [ ] Implement `migrate_profiles_to_model_cards(config: &mut Config)`:
  - Collect profiles from two sources:
    a) `profiles.d/` — parse each `.toml` as `ProfileDef`; skip if name
       matches a built-in AND params equal `Profile::params()`
    b) `config.custom_profiles` — all entries (these are always custom)
  - Load all model cards from `configs.d/`
  - For each card × each collected profile: skip if key exists; else insert
  - Save each modified card
  - Remove processed `.toml` files from `profiles.d/`; rmdir if empty
  - Set `config.custom_profiles = None`; save config (strips the field)
- [ ] Run tests, verify they pass
- [ ] Commit

---

### Task 3: Wire migration into Config::load_from, remove profiles.d generation

**Files:**
- Modify: `crates/kronk-core/src/config/loader.rs`
- Test: inline tests

**Steps:**
- [ ] Write test: `Config::load_from` on a fresh directory does NOT create
  `profiles.d/`
- [ ] Write test: `Config::load_from` with existing `profiles.d/` and a
  model card in `configs.d/` — after load, profile is in the card
- [ ] Run tests, verify they fail
- [ ] Replace the `profiles.d` creation block (lines 67-74) with a call to
  `migrate_profiles_to_model_cards(&mut config)`
- [ ] Remove the `profiles_dir()` method (lines 102-110)
- [ ] Run tests, verify they pass
- [ ] Commit

---

### Task 4: Remove `Profile::params()` — profile becomes a pure label

**Files:**
- Modify: `crates/kronk-core/src/profiles.rs`
- Modify: `crates/kronk-core/src/config/defaults.rs`
- Modify: `crates/kronk-core/src/config/resolve.rs`
- Test: inline tests

**Steps:**
- [ ] Write test: `effective_sampling_with_card` with a card that has
  `[sampling.coding]` returns those params (no built-in merge)
- [ ] Write test: `effective_sampling_with_card` with a card that has no
  sampling for the active profile returns `None` (no fallback)
- [ ] Run tests, verify they fail
- [ ] Remove `Profile::params()` method from the enum
- [ ] Simplify `resolve_profile_params` in `defaults.rs`: always return
  `None` (or delete entirely — profile labels no longer resolve to params)
- [ ] Update `effective_sampling_with_card` in `resolve.rs`:
  - Remove layer 1 (built-in base). Start directly from model card lookup
  - `card.sampling_for(profile_name)` merged with server overrides
- [ ] Update `effective_sampling` similarly (no card = only server overrides)
- [ ] Fix compilation; update/remove affected tests
- [ ] Run `cargo test --workspace`, verify all pass
- [ ] Commit

---

### Task 5: Remove `Profile::Custom` and `custom_profiles` from Config

**Files:**
- Modify: `crates/kronk-core/src/profiles.rs`
- Modify: `crates/kronk-core/src/config/types.rs`
- Modify: `crates/kronk-core/src/config/loader.rs`
- Modify: `crates/kronk-core/src/config/defaults.rs`
- Test: inline tests

**Steps:**
- [ ] Remove `Profile::Custom { name: String }` variant from the enum
- [ ] Update `Profile::FromStr`: unknown names return an error (change
  `type Err` from `Infallible` to a real error type)
- [ ] Update `Profile::Display`: remove `Custom` arm
- [ ] Remove `custom_profiles` field from `Config` in `types.rs`
- [ ] Remove `custom_profiles: None` from `Config::default()` in `loader.rs`
- [ ] Remove `Custom` branch from `resolve_profile_params` (if it still
  exists) or delete `resolve_profile_params` entirely
- [ ] Fix compilation errors across the workspace
- [ ] Run `cargo test --workspace`, verify all pass
- [ ] Commit

---

### Task 6: Remove `ProfileDef`, `load_profiles_d`, `generate_default_profiles`

**Files:**
- Modify: `crates/kronk-core/src/profiles.rs`
- Test: inline tests

**Steps:**
- [ ] Ensure migration code in `migrate.rs` has its own inline TOML parsing
  (or uses `SamplingParams` directly) and no longer depends on
  `ProfileDef` or `load_profiles_d`
- [ ] Remove `ProfileDef` struct
- [ ] Remove `load_profiles_d` function
- [ ] Remove `generate_default_profiles` function
- [ ] Remove tests: `test_load_profiles_d_empty`,
  `test_load_profiles_d_nonexistent`,
  `test_generate_and_load_default_profiles`,
  `test_generate_does_not_overwrite_existing`
- [ ] Run `cargo test --package kronk-core`, verify all pass
- [ ] Run `cargo clippy --workspace -- -D warnings`, fix warnings
- [ ] Commit

---

### Task 7: Populate modelcard sampling on pull from `sampling_templates`

**Files:**
- Modify: `crates/kronk-cli/src/commands/model.rs` (in `cmd_pull`)
- Modify: `crates/kronk-core/src/models/card.rs` (add helper)
- Test: inline tests in `card.rs`

**Steps:**
- [ ] Write test: `ModelCard::populate_sampling_from(templates)` on an empty
  card fills `sampling` from the provided HashMap
- [ ] Write test: calling it on a card that already has `[sampling.coding]`
  does NOT overwrite it (only fills missing keys)
- [ ] Run tests, verify they fail
- [ ] Add `ModelCard::populate_sampling_from(&mut self, templates: &HashMap<String, SamplingParams>)`
- [ ] Run tests, verify they pass
- [ ] In `cmd_pull`, when creating a brand-new ModelCard (no community card),
  call `card.populate_sampling_from(&config.sampling_templates)` before
  saving
- [ ] Commit

---

### Task 8: Update profile CLI handler and CLI definition

**Files:**
- Modify: `crates/kronk-cli/src/handlers/profile.rs`
- Modify: `crates/kronk-cli/src/cli.rs`
- Test: `cargo clippy` + `cargo test`

**Steps:**
- [ ] Remove `ProfileCommands::Add` and `ProfileCommands::Remove` from
  the enum in `cli.rs`
- [ ] Update `Set`'s help text: remove "or a custom name"
- [ ] In `handlers/profile.rs`:
  - `List`: remove all `profiles_dir` / `disk_profiles` / `load_profiles_d`
    / `custom_profiles` code. Show the 4 profile names. Show
    `sampling_templates` values from config as "defaults for new models".
  - `Set`: only accept the 4 built-in names; unknown names error.
  - Remove `Add` and `Remove` match arms entirely.
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`, verify all pass
- [ ] Commit

---

### Task 9: Clean up re-exports, unused imports, and docs

**Files:**
- Modify: `crates/kronk-core/src/config/mod.rs`
- Modify: `README.md`

**Steps:**
- [ ] Remove `pub use defaults::resolve_profile_params` from `config/mod.rs`
  if deleted or no longer public
- [ ] Remove any dangling `use` statements for deleted functions
- [ ] Update `README.md`: remove `profiles.d/` from directory tree, update
  prose about profiles/sampling to reflect the model-card-centric approach
- [ ] Run `cargo check --workspace && cargo clippy --workspace -- -D warnings && cargo test --workspace`
- [ ] Commit
