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

#[derive(Debug, Deserialize)]
struct GithubCommit {
    sha: String,
}

/// Check the latest release version for a backend.
///
/// For llama.cpp: uses /releases/latest (they have stable releases).
/// For ik_llama: uses the latest commit on main, since ik_llama doesn't
/// publish proper releases (only a single stale pre-release tag).
pub async fn check_latest_version(backend: &BackendType) -> Result<String> {
    let client = Client::builder()
        .user_agent("kronk-backend-manager")
        .build()?;

    let token = github_token();

    match backend {
        BackendType::LlamaCpp => {
            let url =
                "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";
            let mut request = client.get(url);
            if let Some(t) = token.as_deref() {
                request = request.header("Authorization", format!("Bearer {}", t));
            }
            let response = request
                .send()
                .await
                .with_context(|| format!("Failed to fetch from {}", url))?;
            check_rate_limit(&response)?;
            let release: GithubRelease = response.json().await?;
            Ok(release.tag_name)
        }
        BackendType::IkLlama => {
            // ik_llama doesn't publish proper releases — their only release tag
            // is a stale pre-release. Use the latest commit SHA on main instead.
            let url =
                "https://api.github.com/repos/ikawrakow/ik_llama.cpp/commits/main";
            let mut request = client.get(url);
            if let Some(t) = token.as_deref() {
                request = request.header("Authorization", format!("Bearer {}", t));
            }
            let response = request
                .send()
                .await
                .with_context(|| format!("Failed to fetch from {}", url))?;
            check_rate_limit(&response)?;
            let commit: GithubCommit = response.json().await?;
            // Return "main" as the version — the actual SHA is for display/comparison
            // but we clone "main" branch for source builds
            Ok(format!("main@{}", &commit.sha[..8]))
        }
        BackendType::Custom => Err(anyhow!("Cannot check updates for custom backends")),
    }
}

fn check_rate_limit(response: &reqwest::Response) -> Result<()> {
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err(anyhow!(
            "GitHub API request failed with 403 Forbidden. \
             This may be due to rate limiting (60 requests/hour for unauthenticated requests). \
             Set GITHUB_TOKEN environment variable for increased rate limits (5000 requests/hour)."
        ));
    }
    if !response.status().is_success() {
        return Err(anyhow!("GitHub API request failed: {}", response.status()));
    }
    Ok(())
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

    // Validate backend exists before installing to prevent orphaned files
    registry
        .get(backend_name)
        .ok_or_else(|| anyhow!("Backend '{}' not found", backend_name))?;

    registry.update_version(
        backend_name,
        resolved_version,
        new_binary_path,
        Some(source),
    )?;

    tracing::info!("Update complete!");
    Ok(())
}
