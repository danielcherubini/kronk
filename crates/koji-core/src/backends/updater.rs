use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;

use super::installer::{install_backend_with_progress, InstallOptions};
use super::registry::{BackendInfo, BackendRegistry, BackendType};
use super::ProgressSink;

/// Check for GitHub token for authenticated API requests (5000 req/hour vs 60 unauth)
fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN").ok()
}

#[derive(Debug, Deserialize)]
pub(super) struct GithubRelease {
    pub(super) tag_name: String,
    #[allow(dead_code)]
    pub(super) prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct GithubCommit {
    sha: String,
}

/// Find the latest non-prerelease release from a list of GitHub releases.
///
/// Releases are expected to be sorted by creation date (newest first).
/// Returns an error if no stable (non-prerelease) release is found.
pub(super) fn find_latest_stable_release(releases: &[GithubRelease]) -> Result<String> {
    let latest_stable = releases
        .iter()
        .find(|r| !r.prerelease)
        .ok_or_else(|| anyhow!("No stable (non-prerelease) releases found"))?;
    Ok(latest_stable.tag_name.clone())
}

/// Check the latest release version for a backend.
///
/// For llama.cpp: uses /releases to filter out pre-release tags.
/// For ik_llama: uses the latest commit on main, since ik_llama doesn't
/// publish proper releases (only a single stale pre-release tag).
pub async fn check_latest_version(backend: &BackendType) -> Result<String> {
    let client = Client::builder()
        .user_agent("koji-backend-manager")
        .build()?;

    let token = github_token();

    match backend {
        BackendType::LlamaCpp => {
            // Use /releases endpoint to filter out pre-release tags.
            // GitHub's /releases/latest may return a pre-release, which is
            // not what we want for stable version checks.
            let url = "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=100";
            let mut request = client.get(url);
            if let Some(t) = token.as_deref() {
                request = request.header("Authorization", format!("Bearer {}", t));
            }
            let response = request
                .send()
                .await
                .with_context(|| format!("Failed to fetch from {}", url))?;
            check_rate_limit(&response)?;
            let releases: Vec<GithubRelease> = response.json().await?;
            find_latest_stable_release(&releases)
        }
        BackendType::IkLlama => {
            // ik_llama doesn't publish proper releases — their only release tag
            // is a stale pre-release. Use the latest commit SHA on main instead.
            let url = "https://api.github.com/repos/ikawrakow/ik_llama.cpp/commits/main";
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
        BackendType::TtsKokoro => Err(anyhow!("Cannot check updates for TTS backends")),
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

/// Update a backend with progress tracking.
pub async fn update_backend_with_progress(
    registry: &mut BackendRegistry,
    backend_name: &str,
    options: InstallOptions,
    latest_version: String,
    progress: Option<Arc<dyn ProgressSink>>,
) -> Result<()> {
    // Validate backend exists before installing to prevent orphaned files
    registry
        .get(backend_name)?
        .ok_or_else(|| anyhow!("Backend '{}' not found", backend_name))?;

    // Clone source before install_backend moves options
    let source = options.source.clone();
    // Clone backend_type before install_backend moves options
    let backend_type = options.backend_type.clone();

    // Install the new version with progress, using the registry's shared client
    let new_binary_path =
        install_backend_with_progress(options, progress, Some(&registry.client)).await?;

    // Resolve "latest" to actual tag before storing in registry
    let resolved_version = if latest_version.to_lowercase() == "latest" {
        // Fetch the actual latest tag
        let actual_latest = check_latest_version(&backend_type).await?;
        tracing::info!("Resolved 'latest' to actual tag: {}", actual_latest);
        actual_latest
    } else {
        latest_version
    };

    registry.update_version(
        backend_name,
        resolved_version,
        new_binary_path,
        Some(source),
    )?;

    tracing::info!("Update complete!");
    Ok(())
}

/// Update a backend (no progress tracking).
///
/// This is a thin wrapper around `update_backend_with_progress` that passes `None`
/// for the progress sink, preserving the original CLI behavior.
pub async fn update_backend(
    registry: &mut BackendRegistry,
    backend_name: &str,
    options: InstallOptions,
    latest_version: String,
) -> Result<()> {
    update_backend_with_progress(registry, backend_name, options, latest_version, None).await
}

/// Check if a GitHub API response indicates rate limiting.
pub fn is_rate_limited(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::FORBIDDEN
}

/// Determine if an update is available by comparing version strings.
pub fn has_update(current: &str, latest: &str) -> bool {
    current != latest
}

/// Check if a backend type supports update checking.
pub fn supports_update_check(backend_type: &BackendType) -> bool {
    matches!(backend_type, BackendType::LlamaCpp | BackendType::IkLlama)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_rate_limited tests ─────────────────────────────────────────────

    #[test]
    fn test_is_rate_limited_forbidden() {
        assert!(is_rate_limited(reqwest::StatusCode::FORBIDDEN));
    }

    #[test]
    fn test_is_rate_limited_not_rate_limited() {
        assert!(!is_rate_limited(reqwest::StatusCode::OK));
        assert!(!is_rate_limited(reqwest::StatusCode::NOT_FOUND));
        assert!(!is_rate_limited(reqwest::StatusCode::TOO_MANY_REQUESTS));
    }

    // ── has_update tests ──────────────────────────────────────────────────

    #[test]
    fn test_has_update_newer_version_available() {
        assert!(has_update("1.0.0", "1.1.0"));
        assert!(has_update("1.0.0", "2.0.0"));
    }

    #[test]
    fn test_has_update_no_update_available() {
        assert!(!has_update("1.0.0", "1.0.0"));
    }

    #[test]
    fn test_has_update_older_version() {
        // Even if latest is older, versions differ so update_available is true
        assert!(has_update("2.0.0", "1.0.0"));
    }

    #[test]
    fn test_has_update_empty_strings() {
        assert!(has_update("", "1.0.0"));
        assert!(!has_update("", ""));
    }

    // ── supports_update_check tests ───────────────────────────────────────

    #[test]
    fn test_supports_update_check_llamacpp() {
        assert!(supports_update_check(&BackendType::LlamaCpp));
    }

    #[test]
    fn test_supports_update_check_ikllama() {
        assert!(supports_update_check(&BackendType::IkLlama));
    }

    #[test]
    fn test_supports_update_check_custom() {
        assert!(!supports_update_check(&BackendType::Custom));
    }

    // ── UpdateCheck construction tests ────────────────────────────────────

    #[test]
    fn test_update_check_construction() {
        let check = UpdateCheck {
            current_version: "1.0.0".to_string(),
            latest_version: "1.1.0".to_string(),
            update_available: true,
        };
        assert_eq!(check.current_version, "1.0.0");
        assert_eq!(check.latest_version, "1.1.0");
        assert!(check.update_available);
    }

    #[test]
    fn test_update_check_no_update() {
        let check = UpdateCheck {
            current_version: "1.0.0".to_string(),
            latest_version: "1.0.0".to_string(),
            update_available: false,
        };
        assert!(!check.update_available);
    }

    // ── github_token tests ────────────────────────────────────────────────

    #[test]
    fn test_github_token_not_set() {
        std::env::remove_var("GITHUB_TOKEN");
        assert!(github_token().is_none());
    }

    // ── find_latest_stable_release tests ────────────────────────────────────

    #[test]
    fn test_find_latest_stable_release_skips_prereleases() {
        let releases = vec![
            GithubRelease {
                tag_name: "v1.0.0-rc1".to_string(),
                prerelease: true,
            },
            GithubRelease {
                tag_name: "v1.0.0-beta2".to_string(),
                prerelease: true,
            },
            GithubRelease {
                tag_name: "v0.9.0".to_string(),
                prerelease: false,
            },
        ];
        let result = find_latest_stable_release(&releases).unwrap();
        assert_eq!(result, "v0.9.0");
    }

    #[test]
    fn test_find_latest_stable_release_no_prereleases() {
        let releases = vec![
            GithubRelease {
                tag_name: "v1.0.0".to_string(),
                prerelease: false,
            },
            GithubRelease {
                tag_name: "v0.9.0".to_string(),
                prerelease: false,
            },
        ];
        let result = find_latest_stable_release(&releases).unwrap();
        assert_eq!(result, "v1.0.0");
    }

    #[test]
    fn test_find_latest_stable_release_all_prereleases() {
        let releases = vec![
            GithubRelease {
                tag_name: "v2.0.0-alpha".to_string(),
                prerelease: true,
            },
            GithubRelease {
                tag_name: "v2.0.0-beta".to_string(),
                prerelease: true,
            },
        ];
        let result = find_latest_stable_release(&releases);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No stable"));
    }

    #[test]
    fn test_find_latest_stable_release_empty_list() {
        let releases: Vec<GithubRelease> = vec![];
        let result = find_latest_stable_release(&releases);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_latest_stable_release_single_stable() {
        let releases = vec![GithubRelease {
            tag_name: "v1.0.0".to_string(),
            prerelease: false,
        }];
        let result = find_latest_stable_release(&releases).unwrap();
        assert_eq!(result, "v1.0.0");
    }
}
