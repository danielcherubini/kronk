pub mod card;
pub mod download;
pub mod pull;
pub mod registry;
pub mod search;
pub mod update;
pub mod verify;

pub use card::{ModelCard, ModelMeta, QuantInfo};
pub use pull::infer_quant_from_filename;
pub use registry::{InstalledModel, ModelRegistry};
pub use search::{search_models, SearchResult, SortBy};

/// Append a HuggingFace `repo_id` (e.g. `"org/repo-name"`) to a base path using
/// the platform-native separator.
///
/// `PathBuf::join("org/repo")` does **not** split on `/` on Windows, producing
/// mixed-slash paths like `C:\models\org/repo`. This function splits on `/` first
/// so the result is always `C:\models\org\repo` on Windows and `/models/org/repo`
/// on Unix.
pub fn repo_path(base: impl Into<std::path::PathBuf>, repo_id: &str) -> std::path::PathBuf {
    repo_id
        .split('/')
        .fold(base.into(), |p, component| p.join(component))
}
