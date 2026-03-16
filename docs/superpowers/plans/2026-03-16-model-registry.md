# Model Registry & HuggingFace Pull Implementation Plan

> **Status: COMPLETED** — Implemented on 2026-03-16 on branch `feat/model-registry`.

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Centralised model management with `~/.kronk/models/` directory, TOML model cards, HuggingFace pull with interactive quant selection, and profile creation from model cards with a 3-layer sampling merge chain.

**Architecture:** Models live in `~/.kronk/models/{company}/{modelname}/` with a `model.toml` card per model. The card defines metadata, per-quant info, and per-use-case sampling overrides. Profiles reference model cards (linked, not copied) so card updates propagate. The `kronk model` subcommand provides `pull`, `ls`, `ps`, `create`, `rm`, and `scan` operations. HuggingFace integration uses the `hf-hub` crate (official Rust client) with `inquire` for interactive quant selection.

**Tech Stack:** Rust, `hf-hub` (tokio feature for async + built-in indicatif progress), `inquire` (interactive prompts), existing `kronk-core` config system, `serde` + `toml` for model cards.

**Sampling merge chain:** `UseCase built-in defaults` → `model card per-use-case overrides` → `profile-level overrides`

---

## File Structure

### New files to create

| File | Responsibility |
|------|---------------|
| `crates/kronk-core/src/models/mod.rs` | Module root, re-exports |
| `crates/kronk-core/src/models/card.rs` | `ModelCard` struct, TOML serde, load/save |
| `crates/kronk-core/src/models/registry.rs` | `ModelRegistry` — scan models dir, list models, find by name |
| `crates/kronk-core/src/models/pull.rs` | HuggingFace API integration — list repo files, filter GGUFs, download |
| `crates/kronk-cli/src/commands/model.rs` | `kronk model` CLI subcommand — pull, ls, ps, create, rm, scan |

### Files to modify

| File | Changes |
|------|---------|
| `crates/kronk-core/src/lib.rs` | Add `pub mod models;` |
| `crates/kronk-core/src/config.rs` | Add `models_dir` to `General`, add `model` + `quant` fields to `ProfileConfig`, update `effective_sampling` for 3-layer merge |
| `crates/kronk-core/Cargo.toml` | Add `hf-hub` dependency |
| `crates/kronk-cli/Cargo.toml` | Add `inquire` dependency |
| `crates/kronk-cli/src/main.rs` | Add `Model` variant to `Commands` enum, wire to `commands::model` |
| `crates/kronk-cli/src/commands/mod.rs` | Add `pub mod model;` |
| `Cargo.toml` | Add `hf-hub`, `inquire`, `tempfile` to workspace dependencies |

---

## Chunk 1: Model Card Data Types & Registry

### Task 1: Add new dependencies to workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/kronk-core/Cargo.toml`
- Modify: `crates/kronk-cli/Cargo.toml`

- [ ] **Step 1: Add workspace dependencies**

In `Cargo.toml` (workspace root), add to `[workspace.dependencies]`:

```toml
hf-hub = { version = "0.5", default-features = false, features = ["tokio"] }
inquire = "0.7"
indicatif = "0.17"
tempfile = "3"
```

- [ ] **Step 2: Add hf-hub + indicatif to kronk-core**

In `crates/kronk-core/Cargo.toml`, add under `[dependencies]`:

```toml
hf-hub.workspace = true
indicatif.workspace = true
```

And under `[dev-dependencies]`:

```toml
tempfile.workspace = true
```

- [ ] **Step 3: Add inquire to kronk-cli**

In `crates/kronk-cli/Cargo.toml`, add under `[dependencies]`:

```toml
inquire.workspace = true
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles with no errors (warnings OK)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/kronk-core/Cargo.toml crates/kronk-cli/Cargo.toml
git commit -m "deps: add hf-hub, inquire, indicatif for model management"
```

---

### Task 2: Define ModelCard types

**Files:**
- Create: `crates/kronk-core/src/models/mod.rs`
- Create: `crates/kronk-core/src/models/card.rs`
- Modify: `crates/kronk-core/src/lib.rs`

- [ ] **Step 1: Write the failing test for ModelCard serde**

Create `crates/kronk-core/src/models/card.rs` with test at bottom:

```rust
use crate::use_cases::SamplingParams;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A model card describing a model and its available quantisations.
/// Lives at `~/.kronk/models/{company}/{modelname}/model.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCard {
    pub model: ModelMeta,
    /// Per-use-case sampling overrides specific to this model.
    /// Keys are use case names: "coding", "chat", "analysis", "creative", or custom names.
    #[serde(default)]
    pub sampling: HashMap<String, SamplingParams>,
    /// Available quantisations. Keys are quant names like "Q4_K_M", "Q8_0".
    #[serde(default)]
    pub quants: HashMap<String, QuantInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelMeta {
    pub name: String,
    /// HuggingFace repo identifier, e.g. "bartowski/OmniCoder-8B-GGUF"
    pub source: String,
    #[serde(default)]
    pub default_context_length: Option<u32>,
    #[serde(default)]
    pub default_gpu_layers: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuantInfo {
    /// Filename of the GGUF file relative to the model directory.
    pub file: String,
    /// File size in bytes (informational).
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Context length override for this specific quant.
    #[serde(default)]
    pub context_length: Option<u32>,
}

impl ModelCard {
    /// Load a model card from a TOML file.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read model card at {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse model card at {}", path.display()))
    }

    /// Save a model card to a TOML file.
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let toml_str = toml::to_string_pretty(self)
            .context("Failed to serialize model card")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }
        std::fs::write(path, &toml_str)
            .with_context(|| format!("Failed to write model card to {}", path.display()))?;
        Ok(())
    }

    /// Get the effective context length for a specific quant.
    /// Falls back to model-level default if the quant doesn't specify one.
    pub fn context_length_for(&self, quant_name: &str) -> Option<u32> {
        self.quants
            .get(quant_name)
            .and_then(|q| q.context_length)
            .or(self.model.default_context_length)
    }

    /// Get model-specific sampling overrides for a given use case name.
    pub fn sampling_for(&self, use_case_name: &str) -> Option<&SamplingParams> {
        self.sampling.get(use_case_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_card_toml() -> &'static str {
        r#"
[model]
name = "OmniCoder"
source = "bartowski/OmniCoder-8B-GGUF"
default_context_length = 8192
default_gpu_layers = 999

[sampling.coding]
temperature = 0.2
top_k = 40

[sampling.chat]
temperature = 0.6

[quants.Q4_K_M]
file = "OmniCoder-8B-Q4_K_M.gguf"
size_bytes = 4_200_000_000
context_length = 8192

[quants.Q8_0]
file = "OmniCoder-8B-Q8_0.gguf"
size_bytes = 8_100_000_000
context_length = 16384
"#
    }

    #[test]
    fn test_model_card_deserialize() {
        let card: ModelCard = toml::from_str(sample_card_toml()).unwrap();
        assert_eq!(card.model.name, "OmniCoder");
        assert_eq!(card.model.source, "bartowski/OmniCoder-8B-GGUF");
        assert_eq!(card.model.default_context_length, Some(8192));
        assert_eq!(card.model.default_gpu_layers, Some(999));
        assert_eq!(card.quants.len(), 2);
        assert_eq!(card.quants["Q4_K_M"].file, "OmniCoder-8B-Q4_K_M.gguf");
        assert_eq!(card.quants["Q8_0"].size_bytes, Some(8_100_000_000));
    }

    #[test]
    fn test_model_card_roundtrip() {
        let card: ModelCard = toml::from_str(sample_card_toml()).unwrap();
        let serialized = toml::to_string_pretty(&card).unwrap();
        let roundtripped: ModelCard = toml::from_str(&serialized).unwrap();
        assert_eq!(card, roundtripped);
    }

    #[test]
    fn test_model_card_sampling_overrides() {
        let card: ModelCard = toml::from_str(sample_card_toml()).unwrap();
        let coding = card.sampling_for("coding").unwrap();
        assert_eq!(coding.temperature, Some(0.2));
        assert_eq!(coding.top_k, Some(40));
        assert_eq!(coding.top_p, None);

        let chat = card.sampling_for("chat").unwrap();
        assert_eq!(chat.temperature, Some(0.6));

        assert!(card.sampling_for("nonexistent").is_none());
    }

    #[test]
    fn test_context_length_for_quant() {
        let card: ModelCard = toml::from_str(sample_card_toml()).unwrap();
        assert_eq!(card.context_length_for("Q8_0"), Some(16384));
        assert_eq!(card.context_length_for("Q4_K_M"), Some(8192));
        assert_eq!(card.context_length_for("unknown"), Some(8192)); // fallback to model default
    }

    #[test]
    fn test_model_card_load_save() {
        let card: ModelCard = toml::from_str(sample_card_toml()).unwrap();
        let dir = std::env::temp_dir().join("kronk_test_model_card");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("model.toml");

        card.save(&path).unwrap();
        let loaded = ModelCard::load(&path).unwrap();
        assert_eq!(card, loaded);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_minimal_model_card() {
        let toml_str = r#"
[model]
name = "TinyModel"
source = "someone/tiny-model-GGUF"
"#;
        let card: ModelCard = toml::from_str(toml_str).unwrap();
        assert_eq!(card.model.name, "TinyModel");
        assert!(card.quants.is_empty());
        assert!(card.sampling.is_empty());
        assert_eq!(card.model.default_context_length, None);
    }
}
```

- [ ] **Step 2: Create the models module**

Create `crates/kronk-core/src/models/mod.rs`:

```rust
pub mod card;
pub mod registry;
pub mod pull;

pub use card::{ModelCard, ModelMeta, QuantInfo};
pub use registry::ModelRegistry;
```

- [ ] **Step 3: Wire into lib.rs**

In `crates/kronk-core/src/lib.rs`, add:

```rust
pub mod models;
```

- [ ] **Step 4: Create stub files so it compiles**

Create `crates/kronk-core/src/models/registry.rs`:

```rust
pub struct ModelRegistry;
```

Create `crates/kronk-core/src/models/pull.rs`:

```rust
// HuggingFace pull integration — implemented in Task 5.
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p kronk-core -- models::card`
Expected: All 6 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/kronk-core/src/models/ crates/kronk-core/src/lib.rs
git commit -m "feat: add ModelCard types with TOML serde and sampling overrides"
```

---

### Task 3: Implement ModelRegistry (scan & list)

**Files:**
- Modify: `crates/kronk-core/src/models/registry.rs`
- Modify: `crates/kronk-core/src/config.rs`

The registry scans the models directory for `model.toml` files and provides lookup/listing.

- [ ] **Step 1: Add `models_dir` to config**

In `crates/kronk-core/src/config.rs`, add a new field to `General`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
}
```

Add a helper method to `Config` impl block:

```rust
/// Resolve the models directory path.
/// Uses `general.models_dir` if set, otherwise defaults to `~/.kronk/models/`.
pub fn models_dir(&self) -> Result<PathBuf> {
    if let Some(ref dir) = self.general.models_dir {
        Ok(PathBuf::from(dir))
    } else {
        let home = directories::UserDirs::new()
            .context("Failed to determine home directory")?;
        Ok(home.home_dir().join(".kronk").join("models"))
    }
}
```

- [ ] **Step 2: Run existing tests to verify nothing broke**

Run: `cargo test -p kronk-core -- config`
Expected: All existing config tests PASS (the new field has `#[serde(default)]` so existing TOML still parses)

- [ ] **Step 3: Write the ModelRegistry implementation with tests**

Replace the stub in `crates/kronk-core/src/models/registry.rs`:

```rust
use crate::models::card::ModelCard;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// An installed model: its location and parsed card.
#[derive(Debug, Clone)]
pub struct InstalledModel {
    /// Directory containing the model files and model.toml
    pub dir: PathBuf,
    /// Parsed model card
    pub card: ModelCard,
    /// Identifier in "company/modelname" format, derived from directory structure
    pub id: String,
}

/// Scans and manages the local model directory.
pub struct ModelRegistry {
    models_dir: PathBuf,
}

impl ModelRegistry {
    pub fn new(models_dir: PathBuf) -> Self {
        Self { models_dir }
    }

    /// Get the base models directory path.
    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Scan the models directory and return all installed models.
    /// Looks for `model.toml` files at `{models_dir}/{company}/{modelname}/model.toml`.
    pub fn scan(&self) -> Result<Vec<InstalledModel>> {
        let mut models = Vec::new();

        if !self.models_dir.exists() {
            return Ok(models);
        }

        for company_entry in std::fs::read_dir(&self.models_dir)
            .with_context(|| format!("Failed to read models directory: {}", self.models_dir.display()))?
        {
            let company_entry = company_entry?;
            let company_path = company_entry.path();
            if !company_path.is_dir() {
                continue;
            }
            let company_name = company_entry.file_name().to_string_lossy().to_string();

            for model_entry in std::fs::read_dir(&company_path)? {
                let model_entry = model_entry?;
                let model_path = model_entry.path();
                if !model_path.is_dir() {
                    continue;
                }
                let model_name = model_entry.file_name().to_string_lossy().to_string();

                let card_path = model_path.join("model.toml");
                if card_path.exists() {
                    match ModelCard::load(&card_path) {
                        Ok(card) => {
                            models.push(InstalledModel {
                                dir: model_path,
                                card,
                                id: format!("{}/{}", company_name, model_name),
                            });
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Skipping malformed model card at {}: {}",
                                card_path.display(),
                                e
                            );
                        }
                    }
                }
            }
        }

        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }

    /// Find a model by its id ("company/modelname").
    pub fn find(&self, id: &str) -> Result<Option<InstalledModel>> {
        let models = self.scan()?;
        Ok(models.into_iter().find(|m| m.id == id))
    }

    /// Get the directory path for a model id. Does not check if the model exists.
    pub fn model_dir(&self, id: &str) -> PathBuf {
        self.models_dir.join(id)
    }

    /// Get the path to the GGUF file for a specific model + quant.
    pub fn gguf_path(&self, id: &str, quant_name: &str) -> Result<Option<PathBuf>> {
        let model = self.find(id)?;
        Ok(model.and_then(|m| {
            m.card.quants.get(quant_name).map(|q| m.dir.join(&q.file))
        }))
    }

    /// Scan for GGUF files in a model directory that aren't tracked in the model card.
    pub fn untracked_ggufs(&self, model_dir: &Path, card: &ModelCard) -> Result<Vec<String>> {
        let tracked: std::collections::HashSet<&str> = card
            .quants
            .values()
            .map(|q| q.file.as_str())
            .collect();

        let mut untracked = Vec::new();
        if model_dir.exists() {
            for entry in std::fs::read_dir(model_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".gguf") && !tracked.contains(name.as_str()) {
                    untracked.push(name);
                }
            }
        }
        untracked.sort();
        Ok(untracked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::card::{ModelCard, ModelMeta, QuantInfo};
    use std::collections::HashMap;

    fn setup_test_dir() -> (tempfile::TempDir, ModelRegistry) {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ModelRegistry::new(tmp.path().to_path_buf());
        (tmp, registry)
    }

    fn create_test_model(base: &Path, company: &str, model: &str) -> ModelCard {
        let model_dir = base.join(company).join(model);
        std::fs::create_dir_all(&model_dir).unwrap();

        let card = ModelCard {
            model: ModelMeta {
                name: model.to_string(),
                source: format!("{}/{}", company, model),
                default_context_length: Some(8192),
                default_gpu_layers: Some(999),
            },
            sampling: HashMap::new(),
            quants: {
                let mut q = HashMap::new();
                q.insert(
                    "Q4_K_M".to_string(),
                    QuantInfo {
                        file: format!("{}-Q4_K_M.gguf", model),
                        size_bytes: Some(4_000_000_000),
                        context_length: None,
                    },
                );
                q
            },
        };
        card.save(&model_dir.join("model.toml")).unwrap();
        std::fs::write(model_dir.join(format!("{}-Q4_K_M.gguf", model)), b"fake").unwrap();
        card
    }

    #[test]
    fn test_scan_empty_dir() {
        let (_, registry) = setup_test_dir();
        let models = registry.scan().unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_scan_nonexistent_dir() {
        let registry = ModelRegistry::new(PathBuf::from("/tmp/kronk_nonexistent_test_dir"));
        let models = registry.scan().unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_scan_finds_models() {
        let (tmp, registry) = setup_test_dir();
        create_test_model(tmp.path(), "bartowski", "OmniCoder");
        create_test_model(tmp.path(), "bartowski", "Llama3");

        let models = registry.scan().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "bartowski/Llama3");
        assert_eq!(models[1].id, "bartowski/OmniCoder");
    }

    #[test]
    fn test_find_model() {
        let (tmp, registry) = setup_test_dir();
        create_test_model(tmp.path(), "bartowski", "OmniCoder");

        let found = registry.find("bartowski/OmniCoder").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().card.model.name, "OmniCoder");

        let not_found = registry.find("bartowski/NotHere").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_gguf_path() {
        let (tmp, registry) = setup_test_dir();
        create_test_model(tmp.path(), "bartowski", "OmniCoder");

        let path = registry.gguf_path("bartowski/OmniCoder", "Q4_K_M").unwrap();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.ends_with("OmniCoder-Q4_K_M.gguf"));
        assert!(path.exists());
    }

    #[test]
    fn test_untracked_ggufs() {
        let (tmp, registry) = setup_test_dir();
        let card = create_test_model(tmp.path(), "bartowski", "OmniCoder");
        let model_dir = tmp.path().join("bartowski").join("OmniCoder");

        std::fs::write(model_dir.join("OmniCoder-Q8_0.gguf"), b"fake").unwrap();

        let untracked = registry.untracked_ggufs(&model_dir, &card).unwrap();
        assert_eq!(untracked, vec!["OmniCoder-Q8_0.gguf"]);
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p kronk-core -- models::registry`
Expected: All 6 registry tests PASS

- [ ] **Step 5: Run all kronk-core tests**

Run: `cargo test -p kronk-core`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add ModelRegistry with scan, find, and untracked GGUF detection"
```

---

### Task 4: Update Config for model-linked profiles

**Files:**
- Modify: `crates/kronk-core/src/config.rs`

- [ ] **Step 1: Write failing test for model-aware profile**

Add to the `tests` module in `crates/kronk-core/src/config.rs`:

```rust
#[test]
fn test_profile_with_model_fields_roundtrip() {
    let profile = ProfileConfig {
        backend: "llama_cpp".to_string(),
        args: vec![],
        use_case: Some(UseCase::Coding),
        sampling: None,
        model: Some("bartowski/OmniCoder".to_string()),
        quant: Some("Q4_K_M".to_string()),
    };
    let toml_str = toml::to_string_pretty(&profile).unwrap();
    let loaded: ProfileConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(loaded.model, Some("bartowski/OmniCoder".to_string()));
    assert_eq!(loaded.quant, Some("Q4_K_M".to_string()));
}

#[test]
fn test_profile_without_model_fields_still_works() {
    let toml_str = r#"
backend = "llama_cpp"
args = ["--host", "0.0.0.0"]
"#;
    let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(profile.model, None);
    assert_eq!(profile.quant, None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kronk-core -- test_profile_with_model`
Expected: FAIL — `model` and `quant` fields don't exist yet

- [ ] **Step 3: Add model/quant fields to ProfileConfig**

In `crates/kronk-core/src/config.rs`, update `ProfileConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub backend: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub use_case: Option<UseCase>,
    #[serde(default)]
    pub sampling: Option<SamplingParams>,
    /// Model card reference in "company/modelname" format.
    #[serde(default)]
    pub model: Option<String>,
    /// Which quant to use from the model card (e.g. "Q4_K_M").
    #[serde(default)]
    pub quant: Option<String>,
}
```

Also update the default `ProfileConfig` in `Config::default()` to include the new fields:

```rust
ProfileConfig {
    backend: "llama_cpp".to_string(),
    args: vec![ /* existing args unchanged */ ]
        .into_iter()
        .map(String::from)
        .collect(),
    use_case: Some(UseCase::Coding),
    sampling: None,
    model: None,
    quant: None,
},
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kronk-core -- config`
Expected: All config tests PASS

- [ ] **Step 5: Write failing test for 3-layer merge**

Add to config.rs tests:

```rust
#[test]
fn test_effective_sampling_with_model_card() {
    use crate::models::card::{ModelCard, ModelMeta, QuantInfo};

    let config = Config::default();

    let mut sampling = HashMap::new();
    sampling.insert("coding".to_string(), SamplingParams {
        temperature: Some(0.2),
        top_k: Some(40),
        ..Default::default()
    });

    let card = ModelCard {
        model: ModelMeta {
            name: "TestModel".to_string(),
            source: "test/model".to_string(),
            default_context_length: None,
            default_gpu_layers: None,
        },
        sampling,
        quants: HashMap::new(),
    };

    let profile = ProfileConfig {
        backend: "test".to_string(),
        args: vec![],
        use_case: Some(UseCase::Coding),
        sampling: Some(SamplingParams {
            top_p: Some(0.85),
            ..Default::default()
        }),
        model: Some("test/model".to_string()),
        quant: None,
    };

    // 3-layer merge: UseCase::Coding (temp=0.3) -> model card (temp=0.2, top_k=40) -> profile (top_p=0.85)
    let params = config.effective_sampling_with_card(&profile, Some(&card)).unwrap();
    assert_eq!(params.temperature, Some(0.2));  // model card override won over use case default
    assert_eq!(params.top_k, Some(40));          // model card override
    assert_eq!(params.top_p, Some(0.85));        // profile override won over everything
    assert_eq!(params.min_p, Some(0.05));        // from UseCase::Coding base (not overridden)
}

#[test]
fn test_effective_sampling_backward_compat() {
    let config = Config::default();
    let profile = ProfileConfig {
        backend: "test".to_string(),
        args: vec![],
        use_case: Some(UseCase::Coding),
        sampling: Some(SamplingParams {
            temperature: Some(0.5),
            ..Default::default()
        }),
        model: None,
        quant: None,
    };
    let params = config.effective_sampling_with_card(&profile, None).unwrap();
    assert_eq!(params.temperature, Some(0.5)); // profile override
    assert_eq!(params.top_k, Some(50));         // from UseCase::Coding
}
```

- [ ] **Step 6: Implement `effective_sampling_with_card`**

Add to the `Config` impl block in `crates/kronk-core/src/config.rs`:

```rust
/// Resolve effective sampling with the 3-layer merge chain:
/// 1. UseCase built-in defaults
/// 2. Model card per-use-case sampling overrides
/// 3. Profile-level sampling overrides
pub fn effective_sampling_with_card(
    &self,
    profile: &ProfileConfig,
    card: Option<&crate::models::card::ModelCard>,
) -> Option<SamplingParams> {
    // Layer 1: Use case base params
    let base = match &profile.use_case {
        Some(UseCase::Custom { name }) => {
            self.custom_use_cases
                .as_ref()
                .and_then(|m| m.get(name))
                .cloned()
        }
        Some(uc) => Some(uc.params()),
        None => None,
    };

    // Layer 2: Model card sampling overrides for this use case
    let use_case_name = profile.use_case.as_ref().map(|uc| uc.to_string());
    let with_model = match (base, card, use_case_name) {
        (Some(base), Some(card), Some(ref uc_name)) => {
            if let Some(model_sampling) = card.sampling_for(uc_name) {
                Some(base.merge(model_sampling))
            } else {
                Some(base)
            }
        }
        (Some(base), _, _) => Some(base),
        (None, Some(card), Some(ref uc_name)) => {
            card.sampling_for(uc_name).cloned()
        }
        (None, _, _) => None,
    };

    // Layer 3: Profile-level overrides
    match (with_model, &profile.sampling) {
        (Some(base), Some(overrides)) => Some(base.merge(overrides)),
        (Some(base), None) => Some(base),
        (None, Some(sampling)) => Some(sampling.clone()),
        (None, None) => None,
    }
}
```

- [ ] **Step 7: Run the new tests**

Run: `cargo test -p kronk-core -- test_effective_sampling_with_model_card test_effective_sampling_backward_compat`
Expected: Both PASS

- [ ] **Step 8: Run all kronk-core tests**

Run: `cargo test -p kronk-core`
Expected: All tests PASS

- [ ] **Step 9: Commit**

```bash
git add crates/kronk-core/src/config.rs
git commit -m "feat: add model/quant fields to ProfileConfig and 3-layer sampling merge"
```

---

## Chunk 2: HuggingFace Pull Integration

### Task 5: Implement HuggingFace repo file listing and download

**Files:**
- Modify: `crates/kronk-core/src/models/pull.rs`

- [ ] **Step 1: Write the pull module**

Replace the stub in `crates/kronk-core/src/models/pull.rs`:

```rust
use anyhow::{Context, Result};
use hf_hub::api::tokio::Api;
use std::path::PathBuf;

/// Information about a GGUF file in a HuggingFace repo.
#[derive(Debug, Clone)]
pub struct RemoteGguf {
    /// Filename, e.g. "OmniCoder-8B-Q4_K_M.gguf"
    pub filename: String,
    /// Inferred quant type from filename, e.g. "Q4_K_M"
    pub quant: Option<String>,
}

/// List GGUF files available in a HuggingFace model repository.
pub async fn list_gguf_files(repo_id: &str) -> Result<Vec<RemoteGguf>> {
    let api = Api::new().context("Failed to initialise HuggingFace API client")?;
    let repo = api.model(repo_id.to_string());
    let info = repo
        .info()
        .await
        .with_context(|| format!("Failed to fetch repo info for '{}'", repo_id))?;

    let ggufs: Vec<RemoteGguf> = info
        .siblings
        .into_iter()
        .filter(|s| s.rfilename.ends_with(".gguf"))
        .map(|s| {
            let quant = infer_quant_from_filename(&s.rfilename);
            RemoteGguf {
                filename: s.rfilename,
                quant,
            }
        })
        .collect();

    Ok(ggufs)
}

/// Download a specific GGUF file from a HuggingFace repo to the given model directory.
/// Returns the local path to the downloaded file.
/// Uses hf-hub's built-in caching + progress bar (indicatif).
pub async fn download_gguf(
    repo_id: &str,
    filename: &str,
    dest_dir: &std::path::Path,
) -> Result<PathBuf> {
    let api = Api::new().context("Failed to initialise HuggingFace API client")?;
    let repo = api.model(repo_id.to_string());

    // hf-hub downloads to its own cache with built-in progress
    let cached_path = repo
        .download(filename)
        .await
        .with_context(|| format!("Failed to download '{}' from '{}'", filename, repo_id))?;

    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("Failed to create model directory: {}", dest_dir.display()))?;

    let dest_path = dest_dir.join(filename);

    // Try hard link first (same filesystem = instant, no extra space)
    // Fall back to copy if hard link fails (cross-filesystem)
    if std::fs::hard_link(&cached_path, &dest_path).is_err() {
        std::fs::copy(&cached_path, &dest_path).with_context(|| {
            format!("Failed to copy downloaded file to {}", dest_path.display())
        })?;
    }

    Ok(dest_path)
}

/// Try to infer the quantisation type from a GGUF filename.
/// Common patterns: "Model-Q4_K_M.gguf", "model.Q8_0.gguf", "model-q4_k_m.gguf"
pub fn infer_quant_from_filename(filename: &str) -> Option<String> {
    let stem = filename.strip_suffix(".gguf")?;

    // Ordered longest-first so "Q4_K_M" matches before "Q4_K"
    let quant_patterns = [
        "IQ2_XXS", "IQ3_XXS",
        "IQ1_S", "IQ1_M", "IQ2_XS", "IQ2_S", "IQ2_M",
        "IQ3_XS", "IQ3_S", "IQ3_M", "IQ4_XS", "IQ4_NL",
        "Q2_K_S", "Q3_K_S", "Q3_K_M", "Q3_K_L",
        "Q4_K_S", "Q4_K_M", "Q4_K_L",
        "Q5_K_S", "Q5_K_M", "Q5_K_L",
        "Q2_K", "Q3_K", "Q4_K", "Q5_K", "Q6_K",
        "Q4_0", "Q4_1", "Q5_0", "Q5_1", "Q6_0", "Q8_0", "Q8_1",
        "F16", "F32", "BF16",
    ];

    let stem_upper = stem.to_uppercase();
    for pattern in &quant_patterns {
        if stem_upper.ends_with(pattern)
            || stem_upper.contains(&format!("-{}", pattern))
            || stem_upper.contains(&format!(".{}", pattern))
            || stem_upper.contains(&format!("_{}", pattern))
        {
            return Some(pattern.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_quant_q4_k_m() {
        assert_eq!(
            infer_quant_from_filename("OmniCoder-8B-Q4_K_M.gguf"),
            Some("Q4_K_M".to_string())
        );
    }

    #[test]
    fn test_infer_quant_q8_0() {
        assert_eq!(
            infer_quant_from_filename("model-Q8_0.gguf"),
            Some("Q8_0".to_string())
        );
    }

    #[test]
    fn test_infer_quant_lowercase() {
        assert_eq!(
            infer_quant_from_filename("model-q4_k_m.gguf"),
            Some("Q4_K_M".to_string())
        );
    }

    #[test]
    fn test_infer_quant_f16() {
        assert_eq!(
            infer_quant_from_filename("model-F16.gguf"),
            Some("F16".to_string())
        );
    }

    #[test]
    fn test_infer_quant_none() {
        assert_eq!(infer_quant_from_filename("model.gguf"), None);
    }

    #[test]
    fn test_infer_quant_dot_separator() {
        assert_eq!(
            infer_quant_from_filename("Llama-3.2-1B-Instruct.Q6_K.gguf"),
            Some("Q6_K".to_string())
        );
    }

    #[test]
    fn test_infer_quant_iq() {
        assert_eq!(
            infer_quant_from_filename("model-IQ4_NL.gguf"),
            Some("IQ4_NL".to_string())
        );
    }
}
```

- [ ] **Step 2: Run the quant inference tests**

Run: `cargo test -p kronk-core -- models::pull`
Expected: All 7 tests PASS

- [ ] **Step 3: Commit**

```bash
git add crates/kronk-core/src/models/pull.rs
git commit -m "feat: add HuggingFace pull module with GGUF listing and quant detection"
```

---

## Chunk 3: CLI — `kronk model` Subcommand

### Task 6: Wire up the `kronk model` subcommand structure

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`
- Modify: `crates/kronk-cli/src/commands/mod.rs`
- Create: `crates/kronk-cli/src/commands/model.rs`

- [ ] **Step 1: Add Model command variant to main.rs**

In `crates/kronk-cli/src/main.rs`, add to the `Commands` enum (after `Config`):

```rust
/// Manage models — pull, list, create profiles
Model {
    #[command(subcommand)]
    command: ModelCommands,
},
```

Add the `ModelCommands` enum after `ConfigCommands`:

```rust
#[derive(Parser, Debug)]
pub enum ModelCommands {
    /// Pull a model from HuggingFace
    Pull {
        /// HuggingFace repo ID, e.g. "bartowski/OmniCoder-8B-GGUF"
        repo: String,
    },
    /// List installed models
    Ls,
    /// Show running model processes
    Ps,
    /// Create a profile from an installed model
    Create {
        /// Profile name to create
        name: String,
        /// Model ID in "company/modelname" format
        #[arg(long)]
        model: String,
        /// Quant to use (e.g. "Q4_K_M"). Interactive picker if omitted.
        #[arg(long)]
        quant: Option<String>,
        /// Use case preset: coding, chat, analysis, creative
        #[arg(long)]
        use_case: Option<String>,
    },
    /// Remove an installed model
    Rm {
        /// Model ID in "company/modelname" format
        model: String,
    },
    /// Scan for untracked GGUF files and update model cards
    Scan,
}
```

Add the match arm in the async block:

```rust
Commands::Model { command } => commands::model::run(&config, command).await,
```

- [ ] **Step 2: Update commands/mod.rs**

In `crates/kronk-cli/src/commands/mod.rs`:

```rust
pub mod model;
```

- [ ] **Step 3: Create the model command handler**

Create `crates/kronk-cli/src/commands/model.rs`. This is the largest single file. It implements each subcommand:

**`cmd_pull`** — Calls `pull::list_gguf_files`, presents `inquire::MultiSelect` picker, downloads selected files, creates/updates model card.

**`cmd_ls`** — Scans registry, prints each model's name/quants/sizes/linked profiles/untracked files.

**`cmd_ps`** — Filters profiles with `model.is_some()`, shows service status + health for each.

**`cmd_create`** — Looks up model in registry, resolves quant (interactive if not specified), builds args with `-m <gguf_path> -c <context> -ngl <layers>`, creates `ProfileConfig` with `model`/`quant` back-references, saves config.

**`cmd_rm`** — Finds model, checks for linked profiles (error if any exist), confirms with `inquire::Confirm`, removes directory.

**`cmd_scan`** — Scans for untracked GGUFs in existing model dirs (adds to card), and for model directories without `model.toml` (creates cards from discovered GGUFs).

```rust
use anyhow::{Context, Result};
use kronk_core::config::Config;
use kronk_core::models::{ModelCard, ModelMeta, ModelRegistry, QuantInfo};
use kronk_core::models::pull;

use crate::ModelCommands;

pub async fn run(config: &Config, command: ModelCommands) -> Result<()> {
    match command {
        ModelCommands::Pull { repo } => cmd_pull(config, &repo).await,
        ModelCommands::Ls => cmd_ls(config),
        ModelCommands::Ps => cmd_ps(config).await,
        ModelCommands::Create { name, model, quant, use_case } => {
            cmd_create(config, &name, &model, quant, use_case).await
        }
        ModelCommands::Rm { model } => cmd_rm(config, &model),
        ModelCommands::Scan => cmd_scan(config),
    }
}

async fn cmd_pull(config: &Config, repo_id: &str) -> Result<()> {
    println!("Pull the lever!");
    println!();
    println!("  Fetching file list from {}...", repo_id);

    let ggufs = pull::list_gguf_files(repo_id).await?;

    if ggufs.is_empty() {
        anyhow::bail!("No GGUF files found in repository '{}'", repo_id);
    }

    let options: Vec<String> = ggufs
        .iter()
        .map(|g| {
            let quant_label = g.quant.as_deref().unwrap_or("unknown");
            format!("{} ({})", g.filename, quant_label)
        })
        .collect();

    let selected = inquire::MultiSelect::new(
        "Which quants do you want to download?",
        options.clone(),
    )
    .with_help_message("Space to select, Enter to confirm")
    .prompt()
    .context("Interactive selection cancelled")?;

    if selected.is_empty() {
        println!("No files selected. Nothing to do.");
        return Ok(());
    }

    let models_dir = config.models_dir()?;
    let model_dir = models_dir.join(repo_id);
    std::fs::create_dir_all(&model_dir)
        .with_context(|| format!("Failed to create directory: {}", model_dir.display()))?;

    let card_path = model_dir.join("model.toml");
    let mut card = if card_path.exists() {
        ModelCard::load(&card_path)?
    } else {
        let name = repo_id.split('/').last().unwrap_or(repo_id).to_string();
        ModelCard {
            model: ModelMeta {
                name,
                source: repo_id.to_string(),
                default_context_length: Some(8192),
                default_gpu_layers: Some(999),
            },
            sampling: std::collections::HashMap::new(),
            quants: std::collections::HashMap::new(),
        }
    };

    for display_str in &selected {
        let idx = options.iter().position(|o| o == display_str).unwrap();
        let gguf = &ggufs[idx];

        println!();
        println!("  Downloading {}...", gguf.filename);

        let local_path = pull::download_gguf(repo_id, &gguf.filename, &model_dir).await?;

        let size_bytes = std::fs::metadata(&local_path).map(|m| m.len()).ok();
        let quant_name = gguf.quant.clone().unwrap_or_else(|| gguf.filename.clone());

        card.quants.insert(
            quant_name.clone(),
            QuantInfo {
                file: gguf.filename.clone(),
                size_bytes,
                context_length: None,
            },
        );

        println!("  Downloaded: {}", local_path.display());
    }

    card.save(&card_path)?;

    println!();
    println!("Oh yeah, it's all coming together.");
    println!("  Model card saved: {}", card_path.display());
    println!();
    println!("  Create a profile:");
    println!("    kronk model create my-profile --model {} --use-case coding", repo_id);

    Ok(())
}

fn cmd_ls(config: &Config) -> Result<()> {
    let models_dir = config.models_dir()?;
    let registry = ModelRegistry::new(models_dir);
    let models = registry.scan()?;

    if models.is_empty() {
        println!("No models installed.");
        println!();
        println!("Pull one:  kronk model pull <huggingface-repo>");
        return Ok(());
    }

    println!("Installed models:");
    println!("{}", "-".repeat(60));

    for model in &models {
        println!();
        println!("  {}  ({})", model.id, model.card.model.name);
        if let Some(ctx) = model.card.model.default_context_length {
            print!("    context: {}  ", ctx);
        }
        if let Some(ngl) = model.card.model.default_gpu_layers {
            print!("gpu-layers: {}", ngl);
        }
        println!();

        if model.card.quants.is_empty() {
            println!("    (no quants)");
        } else {
            for (qname, qinfo) in &model.card.quants {
                let size_str = qinfo.size_bytes
                    .map(|b| format_size(b))
                    .unwrap_or_else(|| "?".to_string());
                println!("    {} -- {} ({})", qname, qinfo.file, size_str);
            }
        }

        let linked_profiles: Vec<&str> = config.profiles.iter()
            .filter(|(_, p)| p.model.as_deref() == Some(&model.id))
            .map(|(name, _)| name.as_str())
            .collect();
        if !linked_profiles.is_empty() {
            println!("    profiles: {}", linked_profiles.join(", "));
        }

        let untracked = registry.untracked_ggufs(&model.dir, &model.card).unwrap_or_default();
        if !untracked.is_empty() {
            println!("    untracked: {}", untracked.join(", "));
        }
    }

    println!();
    Ok(())
}

async fn cmd_ps(config: &Config) -> Result<()> {
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    let model_profiles: Vec<(&str, &kronk_core::config::ProfileConfig)> = config.profiles.iter()
        .filter(|(_, p)| p.model.is_some())
        .map(|(n, p)| (n.as_str(), p))
        .collect();

    if model_profiles.is_empty() {
        println!("No model-based profiles.");
        println!();
        println!("Create one:  kronk model create <name> --model <id> --use-case coding");
        return Ok(());
    }

    println!("Model processes:");
    println!("{}", "-".repeat(60));

    for (name, profile) in model_profiles {
        let model_id = profile.model.as_deref().unwrap_or("?");
        let quant = profile.quant.as_deref().unwrap_or("?");
        let use_case = profile.use_case.as_ref()
            .map(|uc| uc.to_string())
            .unwrap_or_else(|| "none".to_string());

        let service_name = Config::service_name(name);
        let service_status = {
            #[cfg(target_os = "windows")]
            { kronk_core::platform::windows::query_service(&service_name).unwrap_or_else(|_| "UNKNOWN".to_string()) }
            #[cfg(target_os = "linux")]
            { kronk_core::platform::linux::query_service(&service_name).unwrap_or_else(|_| "UNKNOWN".to_string()) }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            { let _ = &service_name; "N/A".to_string() }
        };

        let backend = config.backends.get(&profile.backend);
        let health = if let Some(url) = backend.and_then(|b| b.health_check_url.as_ref()) {
            match http_client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => "HEALTHY",
                _ => "DOWN",
            }
        } else { "N/A" };

        println!();
        println!("  {}  {} / {}", name, model_id, quant);
        println!("    use-case: {}  service: {}  health: {}", use_case, service_status, health);
    }

    println!();
    Ok(())
}

async fn cmd_create(
    config: &Config,
    name: &str,
    model_id: &str,
    quant: Option<String>,
    use_case: Option<String>,
) -> Result<()> {
    let models_dir = config.models_dir()?;
    let registry = ModelRegistry::new(models_dir);

    let installed = registry.find(model_id)?
        .with_context(|| format!("Model '{}' not found. Run `kronk model ls` to see installed models.", model_id))?;

    let quant_name = match quant {
        Some(q) => {
            if !installed.card.quants.contains_key(&q) {
                let available: Vec<&str> = installed.card.quants.keys().map(|s| s.as_str()).collect();
                anyhow::bail!("Quant '{}' not found. Available: {}", q, available.join(", "));
            }
            q
        }
        None => {
            let quant_names: Vec<String> = installed.card.quants.keys().cloned().collect();
            if quant_names.is_empty() {
                anyhow::bail!("No quants available for '{}'. Pull some first.", model_id);
            }
            if quant_names.len() == 1 {
                quant_names.into_iter().next().unwrap()
            } else {
                inquire::Select::new("Select a quant:", quant_names)
                    .prompt()
                    .context("Quant selection cancelled")?
            }
        }
    };

    let resolved_use_case = match use_case {
        Some(uc) => {
            use kronk_core::use_cases::UseCase;
            let parsed = match uc.as_str() {
                "coding" => UseCase::Coding,
                "chat" => UseCase::Chat,
                "analysis" => UseCase::Analysis,
                "creative" => UseCase::Creative,
                custom => UseCase::Custom { name: custom.to_string() },
            };
            Some(parsed)
        }
        None => None,
    };

    let gguf_path = registry.gguf_path(model_id, &quant_name)?
        .with_context(|| format!("GGUF file for quant '{}' not found on disk", quant_name))?;

    let mut args = vec![
        "--host".to_string(), "0.0.0.0".to_string(),
        "-m".to_string(), gguf_path.to_string_lossy().to_string(),
    ];

    if let Some(ctx) = installed.card.context_length_for(&quant_name) {
        args.push("-c".to_string());
        args.push(ctx.to_string());
    }

    if let Some(ngl) = installed.card.model.default_gpu_layers {
        args.push("-ngl".to_string());
        args.push(ngl.to_string());
    }

    let mut config = config.clone();
    if config.profiles.contains_key(name) {
        anyhow::bail!("Profile '{}' already exists. Use `kronk update` or choose a different name.", name);
    }

    let backend_key = config.backends.keys().next().cloned()
        .context("No backends configured. Add one first with `kronk add`.")?;

    config.profiles.insert(
        name.to_string(),
        kronk_core::config::ProfileConfig {
            backend: backend_key.clone(),
            args,
            use_case: resolved_use_case,
            sampling: None,
            model: Some(model_id.to_string()),
            quant: Some(quant_name.clone()),
        },
    );

    config.save()?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Profile:   {}", name);
    println!("  Model:     {}", model_id);
    println!("  Quant:     {}", quant_name);
    println!("  GGUF:      {}", gguf_path.display());
    if let Some(uc) = &config.profiles[name].use_case {
        println!("  Use case:  {}", uc);
    }
    println!();
    println!("Run it:      kronk run --profile {}", name);
    println!("Install it:  kronk service install --profile {}", name);

    Ok(())
}

fn cmd_rm(config: &Config, model_id: &str) -> Result<()> {
    let models_dir = config.models_dir()?;
    let registry = ModelRegistry::new(models_dir);

    let model = registry.find(model_id)?
        .with_context(|| format!("Model '{}' not found.", model_id))?;

    let linked_profiles: Vec<&str> = config.profiles.iter()
        .filter(|(_, p)| p.model.as_deref() == Some(model_id))
        .map(|(name, _)| name.as_str())
        .collect();

    if !linked_profiles.is_empty() {
        anyhow::bail!(
            "Cannot remove '{}': referenced by profiles: {}. Remove those first.",
            model_id, linked_profiles.join(", ")
        );
    }

    let confirm = inquire::Confirm::new(&format!("Remove model '{}' and all its files?", model_id))
        .with_default(false)
        .prompt()
        .context("Confirmation cancelled")?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    std::fs::remove_dir_all(&model.dir)
        .with_context(|| format!("Failed to remove: {}", model.dir.display()))?;

    // Clean up empty parent dir
    if let Some(parent) = model.dir.parent() {
        if parent.read_dir().map(|mut d| d.next().is_none()).unwrap_or(false) {
            let _ = std::fs::remove_dir(parent);
        }
    }

    println!("No touchy! Model '{}' removed.", model_id);
    Ok(())
}

fn cmd_scan(config: &Config) -> Result<()> {
    let models_dir = config.models_dir()?;
    let registry = ModelRegistry::new(models_dir.clone());
    let models = registry.scan()?;

    let mut found_any = false;

    // Check existing models for untracked GGUFs
    for model in &models {
        let untracked = registry.untracked_ggufs(&model.dir, &model.card)?;
        if !untracked.is_empty() {
            println!("  {} -- found {} untracked GGUF file(s):", model.id, untracked.len());
            let mut card = model.card.clone();
            for filename in &untracked {
                let quant = pull::infer_quant_from_filename(filename)
                    .unwrap_or_else(|| "unknown".to_string());
                let size_bytes = model.dir.join(filename).metadata().map(|m| m.len()).ok();
                println!("    + {} ({})", filename, quant);
                card.quants.insert(quant, QuantInfo {
                    file: filename.clone(),
                    size_bytes,
                    context_length: None,
                });
            }
            card.save(&model.dir.join("model.toml"))?;
            found_any = true;
        }
    }

    // Scan for directories with GGUFs but no model.toml
    if models_dir.exists() {
        for company_entry in std::fs::read_dir(&models_dir)? {
            let company_entry = company_entry?;
            if !company_entry.path().is_dir() { continue; }
            let company = company_entry.file_name().to_string_lossy().to_string();

            for model_entry in std::fs::read_dir(company_entry.path())? {
                let model_entry = model_entry?;
                let model_path = model_entry.path();
                if !model_path.is_dir() { continue; }
                if model_path.join("model.toml").exists() { continue; }

                let gguf_files: Vec<String> = std::fs::read_dir(&model_path)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".gguf"))
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();

                if !gguf_files.is_empty() {
                    let model_name = model_entry.file_name().to_string_lossy().to_string();
                    let model_id = format!("{}/{}", company, model_name);
                    println!("  {} -- new model with {} GGUF file(s):", model_id, gguf_files.len());

                    let mut quants = std::collections::HashMap::new();
                    for filename in &gguf_files {
                        let quant = pull::infer_quant_from_filename(filename)
                            .unwrap_or_else(|| "unknown".to_string());
                        let size_bytes = model_path.join(filename).metadata().map(|m| m.len()).ok();
                        println!("    + {} ({})", filename, quant);
                        quants.insert(quant, QuantInfo {
                            file: filename.clone(), size_bytes, context_length: None,
                        });
                    }

                    let card = ModelCard {
                        model: ModelMeta {
                            name: model_name,
                            source: model_id,
                            default_context_length: Some(8192),
                            default_gpu_layers: Some(999),
                        },
                        sampling: std::collections::HashMap::new(),
                        quants,
                    };
                    card.save(&model_path.join("model.toml"))?;
                    found_any = true;
                }
            }
        }
    }

    if !found_any {
        println!("No untracked models or GGUF files found.");
    } else {
        println!();
        println!("Oh yeah, it's all coming together. Model cards updated.");
    }

    Ok(())
}

fn format_size(bytes: u64) -> String {
    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    }
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles. Note: `ModelCommands` must be `pub` in main.rs so commands/model.rs can reference it via `crate::ModelCommands`.

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add kronk model subcommand (pull, ls, ps, create, rm, scan)"
```

---

## Chunk 4: Integration & Polish

### Task 7: Update cmd_run to use 3-layer sampling merge

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`

- [ ] **Step 1: Update cmd_run to load model card**

In the `cmd_run` function in `crates/kronk-cli/src/main.rs`, find:

```rust
let args = config.build_args(profile, backend);
```

Replace with:

```rust
// Load model card if profile references a model
let card = if let Some(ref model_id) = profile.model {
    let models_dir = config.models_dir()?;
    let registry = kronk_core::models::ModelRegistry::new(models_dir);
    registry.find(model_id)?.map(|m| m.card)
} else {
    None
};

// Build args with 3-layer sampling merge
let mut args = backend.default_args.clone();
args.extend(profile.args.clone());
if let Some(sampling) = config.effective_sampling_with_card(profile, card.as_ref()) {
    args.extend(sampling.to_args());
}
```

- [ ] **Step 2: Do the same in the Windows service-run path**

In the `win_service_main` function, find:

```rust
let args = config.build_args(prof, backend);
```

Replace with:

```rust
let card = if let Some(ref model_id) = prof.model {
    config.models_dir().ok().and_then(|dir| {
        kronk_core::models::ModelRegistry::new(dir)
            .find(model_id)
            .ok()
            .flatten()
            .map(|m| m.card)
    })
} else {
    None
};
let mut args = backend.default_args.clone();
args.extend(prof.args.clone());
if let Some(sampling) = config.effective_sampling_with_card(prof, card.as_ref()) {
    args.extend(sampling.to_args());
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check --workspace`
Expected: Compiles

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/kronk-cli/src/main.rs
git commit -m "feat: integrate 3-layer sampling merge into run and service paths"
```

---

### Task 8: Update PLAN.md

**Files:**
- Modify: `PLAN.md`

- [ ] **Step 1: Mark model management as completed**

Under `## Completed`, add:

```markdown
- [x] **Model management** — Centralised model registry (~/.kronk/models/), TOML model cards, `kronk model pull/ls/ps/create/rm/scan`, HuggingFace integration, 3-layer sampling merge
```

Remove the old "Model download" line from `## Planned`.

- [ ] **Step 2: Commit**

```bash
git add PLAN.md
git commit -m "docs: update PLAN.md -- model management complete"
```

---

### Task 9: Manual integration testing

These are manual tests — no code changes, just verification.

- [ ] **Step 1:** `cargo run -- model ls` on empty models dir — expect "No models installed."
- [ ] **Step 2:** `cargo run -- model pull bartowski/Llama-3.2-1B-Instruct-GGUF` — interactive picker, download, model card created
- [ ] **Step 3:** `cat ~/.kronk/models/bartowski/Llama-3.2-1B-Instruct-GGUF/model.toml` — valid TOML
- [ ] **Step 4:** `cargo run -- model ls` — shows model with quant info and sizes
- [ ] **Step 5:** `cargo run -- model create llama-coding --model bartowski/Llama-3.2-1B-Instruct-GGUF --use-case coding` — creates profile
- [ ] **Step 6:** `cargo run -- config show` — new profile with model/quant fields
- [ ] **Step 7:** Manually place a GGUF in `~/.kronk/models/test/manual-model/`, run `cargo run -- model scan` — detects and creates card
- [ ] **Step 8:** `cargo run -- model rm bartowski/Llama-3.2-1B-Instruct-GGUF` — error if profiles reference it, otherwise confirms and removes
