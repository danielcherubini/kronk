use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;

use super::installer::{install_backend, InstallOptions};
use super::registry::{BackendInfo, BackendRegistry, BackendType};

/// Check for GitHub token for authenticated API requests (5000 req/hour vs 60 unauth)
fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN").ok()
}

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

    let token = github_token();
    let url = match backend {
        BackendType::LlamaCpp => "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest",
        BackendType::IkLlama => {
            "https://api.github.com/repos/ikawrakow/ik_llama.cpp/releases?per_page=1"
        }
        BackendType::Custom => return Err(anyhow!("Cannot check updates for custom backends")),
    };

    let mut request = client.get(url);
    if let Some(t) = token.as_deref() {
        request = request.header("Authorization", format!("Bearer {}", t));
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("Failed to fetch from {}", url))?;

    if !response.status().is_success() {
        // Check for rate limiting
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow!(
                "GitHub API request failed with 403 Forbidden. \
                 This may be due to rate limiting (60 requests/hour for unauthenticated requests). \
                 Set GITHUB_TOKEN environment variable for increased rate limits (5000 requests/hour)."
            ));
        }
        return Err(anyhow!("GitHub API request failed: {}", response.status()));
    }

    match backend {
        BackendType::LlamaCpp => {
            let release: GithubRelease = response.json().await?;
            Ok(release.tag_name)
        }
        BackendType::IkLlama => {
            // ik_llama only has pre-releases, so /releases/latest returns 404.
            // Fetch all releases and pick the first (most recent).
            let releases: Vec<GithubRelease> = response.json().await?;
            releases
                .first()
                .map(|r| r.tag_name.clone())
                .ok_or_else(|| anyhow!("No releases found for ik_llama"))
        }
        _ => Err(anyhow!("Cannot check updates for custom backends")),
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
    latest_version: String,
) -> Result<()> {
    // Clone source before install_backend moves options
    let source = options.source.clone();
    // Clone backend_type before install_backend moves options
    let backend_type = options.backend_type.clone();

    // Install the new version
    let new_binary_path = install_backend(options).await?;

    // Resolve "latest" to actual tag before storing in registry
    let resolved_version = if latest_version.to_lowercase() == "latest" {
        // Fetch the actual latest tag
        let actual_latest = check_latest_version(&backend_type).await?;
        tracing::info!("Resolved 'latest' to actual tag: {}", actual_latest);
        actual_latest
    } else {
        latest_version
    };

    // Update registry with the known version and source (no re-fetch to avoid TOCTOU race)
    let _backend_info = registry
        .get(backend_name)
        .ok_or_else(|| anyhow!("Backend '{}' not found", backend_name))?;

    registry.update_version(backend_name, resolved_version, new_binary_path, Some(source))?;

    println!("Update complete!");
    Ok(())
}
