use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ── Local types ──────────────────────────────────────────────────────────────

/// Mirrors `koji_core::config::QuantKind`. Used to distinguish model quants
/// from auxiliary files (mmproj) in the wizard's grouping logic.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuantKind {
    #[default]
    Model,
    Mmproj,
}

#[derive(Deserialize, Clone, Debug)]
pub struct QuantEntry {
    pub filename: String,
    pub quant: Option<String>,
    pub size_bytes: Option<i64>,
    #[serde(default)]
    pub kind: QuantKind,
}

#[derive(Clone, Debug)]
pub struct JobProgress {
    pub job_id: String,
    pub filename: String,
    pub status: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

/// Returned by `POST /koji/v1/pulls` (each element of the array)
#[derive(Deserialize, Clone)]
pub struct PullJobEntry {
    pub job_id: String,
    pub filename: String,
    pub status: String,
}

/// SSE event data payload
#[derive(Deserialize, Clone)]
pub struct SsePayload {
    pub job_id: String,
    pub status: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

// ── Wizard step enum ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum WizardStep {
    RepoInput,
    LoadingQuants,
    SelectQuants,
    SetContext,
    Downloading,
    Done,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn format_bytes(bytes: i64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{bytes} B")
    }
}

pub fn step_class(current: &WizardStep, target: &WizardStep, target_idx: usize) -> &'static str {
    let order = [
        WizardStep::RepoInput,
        WizardStep::LoadingQuants,
        WizardStep::SelectQuants,
        WizardStep::SetContext,
        WizardStep::Downloading,
        WizardStep::Done,
    ];
    let current_idx = order.iter().position(|s| s == current).unwrap_or(0);
    if current == target {
        "wizard-step active"
    } else if current_idx > target_idx {
        "wizard-step completed"
    } else {
        "wizard-step"
    }
}

#[allow(dead_code)]
pub fn is_selection_empty(quants: &HashSet<String>, mmprojs: &HashSet<String>) -> bool {
    quants.is_empty() && mmprojs.is_empty()
}

/// Try to infer the quantisation type from a GGUF filename.
/// Common patterns: "Model-Q4_K_M.gguf", "model.Q8_0.gguf", "model-q4_k_m.gguf"
///
/// This is a wrapper around `koji_core::models::infer_quant_from_filename` for
/// the SSR feature. For CSR (client-side), we use a local implementation.
#[cfg(feature = "ssr")]
pub fn infer_quant_from_filename(filename: &str) -> Option<String> {
    koji_core::models::infer_quant_from_filename(filename)
}

/// Try to infer the quantisation type from a GGUF filename.
/// Common patterns: "Model-Q4_K_M.gguf", "model.Q8_0.gguf", "model-q4_k_m.gguf"
///
/// Client-side (CSR) implementation - mirrors the core logic for WASM builds.
#[cfg(not(feature = "ssr"))]
pub fn infer_quant_from_filename(filename: &str) -> Option<String> {
    let stem = filename.strip_suffix(".gguf")?;

    // Ordered longest-first so "Q4_K_M" matches before "Q4_K"
    // Includes UD (Unsloth Dynamic) and APEX variants
    let quant_patterns = [
        // APEX semantic quants (must come before APEX standard patterns)
        "APEX-I-BALANCED",
        "APEX-I-QUALITY",
        "APEX-I-COMPACT",
        "APEX-I-MINI",
        // APEX IQ quants
        "APEX-IQ2_XXS",
        "APEX-IQ3_XXS",
        "APEX-IQ1_S",
        "APEX-IQ1_M",
        "APEX-IQ2_XS",
        "APEX-IQ2_S",
        "APEX-IQ2_M",
        "APEX-IQ3_XS",
        "APEX-IQ3_S",
        "APEX-IQ3_M",
        "APEX-IQ4_XS",
        "APEX-IQ4_NL",
        // APEX standard quants
        "APEX-Q2_K_S",
        "APEX-Q3_K_S",
        "APEX-Q3_K_M",
        "APEX-Q3_K_L",
        "APEX-Q4_K_S",
        "APEX-Q4_K_M",
        "APEX-Q4_K_L",
        "APEX-Q5_K_S",
        "APEX-Q5_K_M",
        "APEX-Q5_K_L",
        "APEX-Q6_K",
        "APEX-Q8_0",
        // UD semantic quants (must come before UD standard patterns)
        "UD-I-BALANCED",
        "UD-I-QUALITY",
        "UD-I-COMPACT",
        "UD-I-MINI",
        // Unsloth Dynamic (UD) IQ quants
        "UD-IQ2_XXS",
        "UD-IQ3_XXS",
        "UD-IQ1_S",
        "UD-IQ1_M",
        "UD-IQ2_XS",
        "UD-IQ2_S",
        "UD-IQ2_M",
        "UD-IQ3_XS",
        "UD-IQ3_S",
        "UD-IQ3_M",
        "UD-IQ4_XS",
        "UD-IQ4_NL",
        // Unsloth Dynamic (UD) standard quants
        "UD-Q2_K_S",
        "UD-Q3_K_S",
        "UD-Q3_K_M",
        "UD-Q3_K_L",
        "UD-Q4_K_S",
        "UD-Q4_K_M",
        "UD-Q4_K_L",
        "UD-Q5_K_S",
        "UD-Q5_K_M",
        "UD-Q5_K_L",
        "UD-Q6_K",
        "UD-Q8_0",
        // Standard quants
        "IQ4_NL",
        "IQ3_NL",
        "IQ2_NL",
        "Q8_0",
        "Q6_K",
        "Q5_K_L",
        "Q5_K_M",
        "Q5_K_S",
        "Q4_K_L",
        "Q4_K_M",
        "Q4_K_S",
        "Q3_K_L",
        "Q3_K_M",
        "Q3_K_S",
        "Q2_K_S",
        "Q2_K",
        "IQ4_XS",
        "IQ3_XS",
        "IQ3_S",
        "IQ3_M",
        "IQ2_S",
        "IQ2_XS",
        "IQ1_S",
        "IQ1_M",
        "IQ2_XXS",
        "IQ3_XXS",
    ];

    let stem_upper = stem.to_uppercase();

    for pattern in &quant_patterns {
        if stem_upper.contains(pattern) {
            return Some((*pattern).to_string());
        }
    }

    None
}

// ── Request body type ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct PullRequest {
    pub repo_id: String,
    pub quants: Vec<QuantRequest>,
}

#[derive(Serialize)]
pub struct QuantRequest {
    pub filename: String,
    pub quant: Option<String>,
    pub context_length: u32,
}

// ── Public types ─────────────────────────────────────────────────────────────

/// A quant that was successfully downloaded by the wizard. Emitted via the
/// `on_complete` callback so the host can merge new quants into its own state.
#[derive(Clone, Debug)]
pub struct CompletedQuant {
    #[allow(dead_code)]
    pub repo_id: String,
    pub filename: String,
    pub quant: Option<String>,
    pub size_bytes: Option<u64>,
    pub context_length: u32,
}

pub mod components;
