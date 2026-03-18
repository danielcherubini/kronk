use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;

use super::installer::{install_backend, InstallOptions};
use super::registry::{BackendInfo, BackendRegistry, BackendType};

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[allow(dead_code)]
    prerelease: bool,
}

/// Check the latest release version for a backend.
///
/// For llama.cpp: uses /releases/latest (they have stable releases).
/// For ik_llama: uses /releases (and picks the first one) because
/// they only have pre-releases, and /releases/latest returns 404
/// when no non-prerelease exists.
pub async fn check_latest_version(backend: &BackendType) -> Result<String> {
    let client = Client::builder()
        .user_agent("kronk-backend-manager")
        .build()?;

    match backend {
        BackendType::LlamaCpp => {
            let url = "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";
            let response = client
                .get(url)
                .send()
                .await
                .with_context(|| "Failed to fetch latest llama.cpp release")?;

            if !response.status().is_success() {
                return Err(anyhow!("GitHub API request failed: {}", response.status()));
            }

            let release: GithubRelease = response.json().await?;
            Ok(release.tag_name)
        }
        BackendType::IkLlama => {
            // ik_llama only has pre-releases, so /releases/latest returns 404.
            // Fetch all releases and pick the first (most recent).
            let url = "https://api.github.com/repos/ikawrakow/ik_llama.cpp/releases?per_page=1";
            let response = client
                .get(url)
                .send()
                .await
                .with_context(|| "Failed to fetch ik_llama releases")?;

            if !response.status().is_success() {
                return Err(anyhow!("GitHub API request failed: {}", response.status()));
            }

            let releases: Vec<GithubRelease> = response.json().await?;
            releases
                .first()
                .map(|r| r.tag_name.clone())
                .ok_or_else(|| anyhow!("No releases found for ik_llama"))
        }
        BackendType::Custom => {
            Err(anyhow!("Cannot check updates for custom backends"))
        }
    }
}

pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
}

pub async fn check_updates(backend_info: &BackendInfo) -> Result<UpdateCheck> {
    let latest = check_latest_version(&backend_info.backend_type).await?;

    Ok(UpdateCheck {
        current_version: backend_info.version.clone(),
        latest_version: latest.clone(),
        update_available: latest != backend_info.version,
    })
}

pub async fn update_backend(
    registry: &mut BackendRegistry,
    backend_name: &str,
    options: InstallOptions,
) -> Result<()> {
    // Install the new version
    let new_binary_path = install_backend(options).await?;

    // Fetch the latest version tag (we need it for the registry)
    let backend_info = registry
        .get(backend_name)
        .ok_or_else(|| anyhow!("Backend '{}' not found", backend_name))?;

    let latest = check_latest_version(&backend_info.backend_type).await?;

    // Update registry in one call
    registry.update_version(backend_name, latest, new_binary_path)?;

    println!("Update complete!");
    Ok(())
}