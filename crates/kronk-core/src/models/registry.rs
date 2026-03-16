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

        for company_entry in std::fs::read_dir(&self.models_dir).with_context(|| {
            format!(
                "Failed to read models directory: {}",
                self.models_dir.display()
            )
        })? {
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
        Ok(model.and_then(|m| m.card.quants.get(quant_name).map(|q| m.dir.join(&q.file))))
    }

    /// Scan for GGUF files in a model directory that aren't tracked in the model card.
    pub fn untracked_ggufs(&self, model_dir: &Path, card: &ModelCard) -> Result<Vec<String>> {
        let tracked: std::collections::HashSet<&str> =
            card.quants.values().map(|q| q.file.as_str()).collect();

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
