//! Shared types for backend management.

use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::gpu::GpuType;

/// Metadata for an installed backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub name: String,
    pub backend_type: BackendType,
    pub version: String,
    pub path: PathBuf,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_type: Option<GpuType>,
    #[serde(default)]
    pub gpu_variant: String,
    #[serde(default)]
    pub source: Option<BackendSource>,
}

/// Source of a backend installation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", content = "content")]
pub enum BackendSource {
    Prebuilt {
        version: String,
    },
    SourceCode {
        version: String,
        git_url: String,
        /// Optional specific commit hash to check out after cloning.
        /// When set, the clone uses enough depth to reach the commit and
        /// then runs `git checkout <commit>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BackendType {
    LlamaCpp,
    IkLlama,
    TtsKokoro,
    Custom,
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::LlamaCpp => write!(f, "llama_cpp"),
            BackendType::IkLlama => write!(f, "ik_llama"),
            BackendType::TtsKokoro => write!(f, "tts_kokoro"),
            BackendType::Custom => write!(f, "custom"),
        }
    }
}

impl BackendType {
    pub fn is_tts(&self) -> bool {
        matches!(self, BackendType::TtsKokoro)
    }
}

impl FromStr for BackendType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "llama_cpp" | "llamacpp" => Ok(BackendType::LlamaCpp),
            "ik_llama" | "ik-llama" | "ikllama" => Ok(BackendType::IkLlama),
            "tts_kokoro" | "ttskokoro" => Ok(BackendType::TtsKokoro),
            "custom" => Ok(BackendType::Custom),
            _ => Err(format!(
                "Unknown backend type '{}'. Supported: llama_cpp, ik_llama, tts_kokoro, custom",
                s
            )),
        }
    }
}
