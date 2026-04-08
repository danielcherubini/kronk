#[allow(unused_imports)]
use axum::{body::Body, http::Request, Router};
#[allow(unused_imports)]
use tower::ServiceExt;

#[cfg(test)]
mod fixtures {
    use std::fs;
    use std::path::Path;

    pub fn read_fixture(name: &str) -> String {
        let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name);
        fs::read_to_string(fixture_path).expect("Failed to read fixture file")
    }
}

#[tokio::test]
#[ignore = "Requires backend registry setup"]
async fn test_get_backends_empty_registry_matches_snapshot() {
    // This test requires a proper backend registry setup
    // Skipping for now - will be implemented after registry is wired
}

#[tokio::test]
#[ignore = "Requires backend registry setup"]
async fn test_get_backends_includes_installed_entry() {
    // This test requires a proper backend registry setup
    // Skipping for now - will be implemented after registry is wired
}

#[tokio::test]
#[ignore = "Requires backend registry setup"]
async fn test_get_backends_custom_entry_appears_in_custom_array() {
    // This test requires a proper backend registry setup
    // Skipping for now - will be implemented after registry is wired
}

#[tokio::test]
#[ignore = "Requires backend registry setup"]
async fn test_get_capabilities_returns_supported_cuda_versions() {
    // This test requires a proper backend registry setup
    // Skipping for now - will be implemented after registry is wired
}

#[tokio::test]
#[ignore = "Requires backend registry setup"]
async fn test_origin_enforcement_blocks_cross_origin_post() {
    // TODO: Task 5 - POST routes don't exist yet
}
