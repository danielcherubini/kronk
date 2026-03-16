use serde::{Deserialize, Serialize};
use std::fmt;

/// Sampling parameters for LLM inference.
/// All fields are Option so that only explicitly-set values
/// get passed to the backend binary as CLI args.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SamplingParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
}

/// Built-in use case presets that auto-configure sampling params.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum UseCase {
    Coding,
    Chat,
    Analysis,
    Creative,
    Custom { name: String },
}

impl UseCase {
    /// Return the preset sampling params for this use case.
    pub fn params(&self) -> SamplingParams {
        match self {
            UseCase::Coding => SamplingParams {
                temperature: Some(0.3),
                top_p: Some(0.9),
                top_k: Some(50),
                min_p: Some(0.05),
                presence_penalty: Some(0.1),
                frequency_penalty: None,
                repeat_penalty: None,
            },
            UseCase::Chat => SamplingParams {
                temperature: Some(0.7),
                top_p: Some(0.95),
                top_k: Some(40),
                min_p: Some(0.05),
                presence_penalty: Some(0.0),
                frequency_penalty: None,
                repeat_penalty: None,
            },
            UseCase::Analysis => SamplingParams {
                temperature: Some(0.3),
                top_p: Some(0.9),
                top_k: Some(20),
                min_p: Some(0.05),
                presence_penalty: Some(0.0),
                frequency_penalty: None,
                repeat_penalty: None,
            },
            UseCase::Creative => SamplingParams {
                temperature: Some(0.9),
                top_p: Some(0.95),
                top_k: Some(50),
                min_p: Some(0.02),
                presence_penalty: Some(0.0),
                frequency_penalty: None,
                repeat_penalty: None,
            },
            UseCase::Custom { .. } => SamplingParams::default(),
        }
    }

    /// List all built-in use cases with descriptions.
    pub fn all() -> Vec<(&'static str, &'static str, UseCase)> {
        vec![
            (
                "coding",
                "Low temp (0.3), deterministic for code gen / agentic tasks",
                UseCase::Coding,
            ),
            (
                "chat",
                "Balanced (temp 0.7) for conversational use",
                UseCase::Chat,
            ),
            (
                "analysis",
                "Deterministic (temp 0.3), focused sampling",
                UseCase::Analysis,
            ),
            (
                "creative",
                "High temp (0.9), diverse and exploratory output",
                UseCase::Creative,
            ),
        ]
    }
}

impl std::str::FromStr for UseCase {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s.to_lowercase().trim() {
            "coding" => UseCase::Coding,
            "chat" => UseCase::Chat,
            "analysis" => UseCase::Analysis,
            "creative" => UseCase::Creative,
            other => UseCase::Custom {
                name: other.to_string(),
            },
        })
    }
}

impl fmt::Display for UseCase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UseCase::Coding => write!(f, "coding"),
            UseCase::Chat => write!(f, "chat"),
            UseCase::Analysis => write!(f, "analysis"),
            UseCase::Creative => write!(f, "creative"),
            UseCase::Custom { name } => write!(f, "{}", name),
        }
    }
}

impl SamplingParams {
    /// Merge two SamplingParams. Values in `overrides` take precedence.
    /// Used for: use_case.params().merge(profile.sampling) → effective params.
    pub fn merge(&self, overrides: &SamplingParams) -> SamplingParams {
        SamplingParams {
            temperature: overrides.temperature.or(self.temperature),
            top_k: overrides.top_k.or(self.top_k),
            top_p: overrides.top_p.or(self.top_p),
            min_p: overrides.min_p.or(self.min_p),
            presence_penalty: overrides.presence_penalty.or(self.presence_penalty),
            frequency_penalty: overrides.frequency_penalty.or(self.frequency_penalty),
            repeat_penalty: overrides.repeat_penalty.or(self.repeat_penalty),
        }
    }

    /// Convert to CLI args for llama.cpp backend.
    /// Only emits flags for fields that are Some.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(v) = self.temperature {
            args.push("--temp".to_string());
            args.push(format!("{:.2}", v));
        }
        if let Some(v) = self.top_k {
            args.push("--top-k".to_string());
            args.push(v.to_string());
        }
        if let Some(v) = self.top_p {
            args.push("--top-p".to_string());
            args.push(format!("{:.2}", v));
        }
        if let Some(v) = self.min_p {
            args.push("--min-p".to_string());
            args.push(format!("{:.2}", v));
        }
        if let Some(v) = self.presence_penalty {
            args.push("--presence-penalty".to_string());
            args.push(format!("{:.2}", v));
        }
        if let Some(v) = self.frequency_penalty {
            args.push("--frequency-penalty".to_string());
            args.push(format!("{:.2}", v));
        }
        if let Some(v) = self.repeat_penalty {
            args.push("--repeat-penalty".to_string());
            args.push(format!("{:.2}", v));
        }
        args
    }

    /// Returns true if all fields are None.
    pub fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.top_k.is_none()
            && self.top_p.is_none()
            && self.min_p.is_none()
            && self.presence_penalty.is_none()
            && self.frequency_penalty.is_none()
            && self.repeat_penalty.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coding_preset() {
        let params = UseCase::Coding.params();
        assert_eq!(params.temperature, Some(0.3));
        assert_eq!(params.top_k, Some(50));
        assert_eq!(params.top_p, Some(0.9));
    }

    #[test]
    fn test_merge_override_wins() {
        let base = UseCase::Coding.params(); // temp 0.3
        let overrides = SamplingParams {
            temperature: Some(0.5), // override
            ..Default::default()
        };
        let merged = base.merge(&overrides);
        assert_eq!(merged.temperature, Some(0.5)); // override won
        assert_eq!(merged.top_k, Some(50)); // base kept
    }

    #[test]
    fn test_merge_none_keeps_base() {
        let base = UseCase::Chat.params();
        let overrides = SamplingParams::default(); // all None
        let merged = base.merge(&overrides);
        assert_eq!(merged, base);
    }

    #[test]
    fn test_to_args_coding() {
        let params = SamplingParams {
            temperature: Some(0.3),
            top_k: Some(50),
            ..Default::default()
        };
        let args = params.to_args();
        assert_eq!(args, vec!["--temp", "0.30", "--top-k", "50"]);
    }

    #[test]
    fn test_to_args_empty() {
        let params = SamplingParams::default();
        assert!(params.to_args().is_empty());
    }

    #[test]
    fn test_use_case_display() {
        assert_eq!(UseCase::Coding.to_string(), "coding");
        assert_eq!(UseCase::Creative.to_string(), "creative");
    }

    #[test]
    fn test_use_case_serde_roundtrip() {
        let uc = UseCase::Coding;
        let json = serde_json::to_string(&uc).unwrap();
        assert_eq!(json, "\"coding\"");
        let back: UseCase = serde_json::from_str(&json).unwrap();
        assert_eq!(back, UseCase::Coding);
    }
}
