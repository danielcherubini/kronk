use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What kind of file a quant entry represents. Mirrors `koji_core::config::QuantKind`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuantKind {
    #[default]
    Model,
    Mmproj,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuantInfo {
    pub file: String,
    #[serde(default)]
    pub kind: QuantKind,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub context_length: Option<u32>,
    // --- DB-enriched fields returned by /api/models/:id ---
    // These are skipped on save so the backend never receives them (it
    // authoritatively owns this data in the SQLite DB).
    #[serde(default, skip_serializing)]
    pub lfs_oid: Option<String>,
    #[serde(default, skip_serializing)]
    pub db_size_bytes: Option<u64>,
    #[serde(default, skip_serializing)]
    pub last_verified_at: Option<String>,
    #[serde(default, skip_serializing)]
    pub verified_ok: Option<bool>,
    #[serde(default, skip_serializing)]
    pub verify_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDetail {
    pub id: String,
    pub backend: String,
    pub model: Option<String>,
    pub quant: Option<String>,
    #[serde(default)]
    pub mmproj: Option<String>,
    pub args: Vec<String>,
    pub sampling: Option<serde_json::Value>,
    pub enabled: bool,
    pub context_length: Option<u32>,
    pub port: Option<u16>,
    pub api_name: Option<String>,
    pub gpu_layers: Option<u32>,
    pub quants: BTreeMap<String, QuantInfo>,
    pub backends: Vec<String>,
    #[serde(default)]
    pub repo_commit_sha: Option<String>,
    #[serde(default)]
    pub repo_pulled_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResponse {
    pub models: Vec<serde_json::Value>,
    pub backends: Vec<String>,
    pub sampling_templates: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SamplingField {
    pub enabled: bool,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelForm {
    pub id: String,
    pub backend: String,
    pub model: Option<String>,
    pub quant: Option<String>,
    pub mmproj: Option<String>,
    pub args: String,
    pub sampling: std::collections::HashMap<String, SamplingField>,
    pub enabled: bool,
    pub context_length: Option<u32>,
    pub port: Option<u16>,
    pub api_name: Option<String>,
    pub gpu_layers: Option<u32>,
    pub quants: BTreeMap<String, QuantInfo>,
}

/// Response from POST /api/models/:id/refresh — surfaces the updated repo
/// commit SHA and the full per-file DB records for merging back into the editor.
#[derive(Debug, Clone, Deserialize)]
pub struct RefreshResponse {
    #[serde(default)]
    pub repo_commit_sha: Option<String>,
    #[serde(default)]
    pub repo_pulled_at: Option<String>,
    #[serde(default)]
    pub files: Vec<FileRecordJson>,
}

/// Response from POST /api/models/:id/verify.
#[derive(Debug, Clone, Deserialize)]
pub struct VerifyResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub any_unknown: bool,
    #[serde(default)]
    pub files: Vec<FileRecordJson>,
}

/// Subset of `ModelFileRecord` as serialized by `file_record_json` in the
/// web backend — carries the DB-authoritative size, LFS hash and verify state
/// for a single file. Used to merge refresh/verify responses back into the
/// editor `quants` signal without a full page reload.
#[derive(Debug, Clone, Deserialize)]
pub struct FileRecordJson {
    pub filename: String,
    #[serde(default)]
    pub lfs_oid: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub last_verified_at: Option<String>,
    #[serde(default)]
    pub verified_ok: Option<bool>,
    #[serde(default)]
    pub verify_error: Option<String>,
}
