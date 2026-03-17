use crate::profiles::SamplingParams;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A model card describing a model and its available quantisations.
/// Lives at `~/.config/kronk/configs.d/<company>-<model>.toml`.
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ModelMeta {
    pub name: String,
    /// HuggingFace repo identifier, e.g. "bartowski/OmniCoder-8B-GGUF"
    pub source: String,
    #[serde(default)]
    pub default_context_length: Option<u32>,
    #[serde(default)]
    pub default_gpu_layers: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
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

pub fn load(path: &std::path::Path) -> anyhow::Result<ModelCard> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read model card at {}", path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("Failed to parse model card at {}", path.display()))
}

pub fn save(card: &ModelCard, path: &std::path::Path) -> anyhow::Result<()> {
    let toml_str = toml::to_string_pretty(card).context("Failed to serialize model card")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    std::fs::write(path, &toml_str)
        .with_context(|| format!("Failed to write model card to {}", path.display()))?;
    Ok(())
}

impl ModelCard {
    /// Load a model card from a TOML file.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        load(path)
    }

    /// Save a model card to a TOML file.
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        save(self, path)
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
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("model.toml");

        super::save(&card, &path).unwrap();
        let loaded = super::load(&path).unwrap();
        assert_eq!(card, loaded);
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
