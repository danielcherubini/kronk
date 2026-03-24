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

/// Built-in profile presets that auto-configure sampling params.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Coding,
    Chat,
    Analysis,
    Creative,
    Custom { name: String },
}

impl Profile {
    /// Return the preset sampling params for this profile.
    pub fn params(&self) -> SamplingParams {
        match self {
            Profile::Coding => SamplingParams {
                temperature: Some(0.3),
                top_p: Some(0.9),
                top_k: Some(50),
                min_p: Some(0.05),
                presence_penalty: Some(0.1),
                frequency_penalty: None,
                repeat_penalty: None,
            },
            Profile::Chat => SamplingParams {
                temperature: Some(0.7),
                top_p: Some(0.95),
                top_k: Some(40),
                min_p: Some(0.05),
                presence_penalty: Some(0.0),
                frequency_penalty: None,
                repeat_penalty: None,
            },
            Profile::Analysis => SamplingParams {
                temperature: Some(0.3),
                top_p: Some(0.9),
                top_k: Some(20),
                min_p: Some(0.05),
                presence_penalty: Some(0.0),
                frequency_penalty: None,
                repeat_penalty: None,
            },
            Profile::Creative => SamplingParams {
                temperature: Some(0.9),
                top_p: Some(0.95),
                top_k: Some(50),
                min_p: Some(0.02),
                presence_penalty: Some(0.0),
                frequency_penalty: None,
                repeat_penalty: None,
            },
            Profile::Custom { .. } => SamplingParams::default(),
        }
    }

    /// List all built-in profiles with descriptions.
    pub fn all() -> Vec<(&'static str, &'static str, Profile)> {
        vec![
            (
                "coding",
                "Low temp (0.3), deterministic for code gen / agentic tasks",
                Profile::Coding,
            ),
            (
                "chat",
                "Balanced (temp 0.7) for conversational use",
                Profile::Chat,
            ),
            (
                "analysis",
                "Deterministic (temp 0.3), focused sampling",
                Profile::Analysis,
            ),
            (
                "creative",
                "High temp (0.9), diverse and exploratory output",
                Profile::Creative,
            ),
        ]
    }
}

impl std::str::FromStr for Profile {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s.to_lowercase().trim() {
            "coding" => Profile::Coding,
            "chat" => Profile::Chat,
            "analysis" => Profile::Analysis,
            "creative" => Profile::Creative,
            other => Profile::Custom {
                name: other.to_string(),
            },
        })
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Profile::Coding => write!(f, "coding"),
            Profile::Chat => write!(f, "chat"),
            Profile::Analysis => write!(f, "analysis"),
            Profile::Creative => write!(f, "creative"),
            Profile::Custom { name } => write!(f, "{}", name),
        }
    }
}

impl SamplingParams {
    /// Merge two SamplingParams. Values in `overrides` take precedence.
    /// Used for: profile.params().merge(server.sampling) → effective params.
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

/// A profile definition loaded from profiles.d/<name>.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileDef {
    pub sampling: SamplingParams,
}

/// Load all profile definitions from the profiles.d directory.
pub fn load_profiles_d(
    dir: &std::path::Path,
) -> anyhow::Result<std::collections::HashMap<String, SamplingParams>> {
    let mut profiles = std::collections::HashMap::new();
    if !dir.exists() {
        return Ok(profiles);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let contents = std::fs::read_to_string(&path)?;
            match toml::from_str::<ProfileDef>(&contents) {
                Ok(def) => {
                    profiles.insert(name, def.sampling);
                }
                Err(e) => {
                    tracing::warn!("Skipping malformed profile {}: {}", path.display(), e);
                }
            }
        }
    }
    Ok(profiles)
}

/// Generate default profile TOML files in the given directory.
/// Does NOT overwrite existing files.
pub fn generate_default_profiles(dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, _desc, profile) in Profile::all() {
        let path = dir.join(format!("{}.toml", name));
        if !path.exists() {
            let def = ProfileDef {
                sampling: profile.params(),
            };
            let toml_str = toml::to_string_pretty(&def)?;
            let comment = match name {
                "coding" => "# Sampling profile for code generation and agentic tasks.\n# Low temperature for deterministic, focused output.\n\n",
                "chat" => "# Sampling profile for conversational use.\n# Balanced temperature for natural responses.\n\n",
                "analysis" => "# Sampling profile for data analysis and reasoning.\n# Low temperature with focused sampling.\n\n",
                "creative" => "# Sampling profile for creative writing and brainstorming.\n# High temperature for diverse, exploratory output.\n\n",
                _ => "",
            };
            std::fs::write(&path, format!("{}{}", comment, toml_str))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coding_preset() {
        let params = Profile::Coding.params();
        assert_eq!(params.temperature, Some(0.3));
        assert_eq!(params.top_k, Some(50));
        assert_eq!(params.top_p, Some(0.9));
    }

    #[test]
    fn test_merge_override_wins() {
        let base = Profile::Coding.params(); // temp 0.3
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
        let base = Profile::Chat.params();
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
    fn test_profile_display() {
        assert_eq!(Profile::Coding.to_string(), "coding");
        assert_eq!(Profile::Creative.to_string(), "creative");
    }

    #[test]
    fn test_profile_serde_roundtrip() {
        let uc = Profile::Coding;
        let json = serde_json::to_string(&uc).unwrap();
        assert_eq!(json, "\"coding\"");
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Profile::Coding);
    }

    #[test]
    fn test_load_profiles_d_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let profiles = super::load_profiles_d(tmp.path()).unwrap();
        assert!(profiles.is_empty());
    }

    #[test]
    fn test_load_profiles_d_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("does_not_exist");
        let profiles = super::load_profiles_d(&nonexistent).unwrap();
        assert!(profiles.is_empty());
    }

    #[test]
    fn test_generate_and_load_default_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        super::generate_default_profiles(tmp.path()).unwrap();

        let profiles = super::load_profiles_d(tmp.path()).unwrap();
        assert!(profiles.contains_key("coding"));
        assert!(profiles.contains_key("chat"));
        assert!(profiles.contains_key("analysis"));
        assert!(profiles.contains_key("creative"));

        let coding = &profiles["coding"];
        assert_eq!(coding.temperature, Some(0.3));
    }

    #[test]
    fn test_generate_does_not_overwrite_existing() {
        let tmp = tempfile::tempdir().unwrap();
        super::generate_default_profiles(tmp.path()).unwrap();

        // Modify coding.toml
        let coding_path = tmp.path().join("coding.toml");
        std::fs::write(&coding_path, "[sampling]\ntemperature = 0.1\n").unwrap();

        // Re-generate should not overwrite
        super::generate_default_profiles(tmp.path()).unwrap();

        let profiles = super::load_profiles_d(tmp.path()).unwrap();
        assert_eq!(profiles["coding"].temperature, Some(0.1));
    }
}
