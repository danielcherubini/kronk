use super::pull::_setup_model_after_pull_with_config;
use super::types::QuantEntry;
use crate::proxy::pull_jobs::{PullJob, PullJobStatus};

/// Verifies that `setup_model_after_pull` creates a model card and config entry.
#[tokio::test]
async fn test_setup_model_creates_card() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().to_path_buf();
    let configs_dir = config_dir.join("configs");
    let models_dir = config_dir.join("models");
    std::fs::create_dir_all(&configs_dir).unwrap();

    let repo_id = "bartowski/Qwen3-8B-GGUF";
    let filename = "Qwen3-8B-Q4_K_M.gguf";
    let repo_slug = repo_id.replace('/', "--");
    // dest_dir uses the two-level org/repo structure (matches production behaviour)
    let dest_dir = models_dir.join(repo_id);
    std::fs::create_dir_all(&dest_dir).unwrap();

    // Write a dummy GGUF file
    std::fs::write(dest_dir.join(filename), b"dummy gguf content").unwrap();

    // Build a config with loaded_from pointing to our temp dir
    let mut config = crate::config::Config {
        loaded_from: Some(config_dir.clone()),
        ..Default::default()
    };
    // Save it so Config::load_from can find it
    config.save_to(&config_dir).unwrap();

    let spec = super::types::QuantDownloadSpec {
        filename: filename.to_string(),
        quant: Some("Q4_K_M".to_string()),
        context_length: Some(8192),
    };

    // Call the inner helper directly (avoids relying on system Config::load())
    _setup_model_after_pull_with_config(&mut config, repo_id, &spec, &dest_dir).await;

    // Assert the card file exists
    let card_path = configs_dir.join(format!("{}.toml", repo_slug));
    assert!(
        card_path.exists(),
        "Expected card file at {}",
        card_path.display()
    );

    // Load and inspect the card
    let card = crate::models::card::ModelCard::load(&card_path).expect("Card should be loadable");
    assert!(
        card.quants.contains_key("Q4_K_M"),
        "Expected Q4_K_M quant in card, got: {:?}",
        card.quants.keys().collect::<Vec<_>>()
    );
    assert_eq!(card.quants["Q4_K_M"].file, filename);
    assert_eq!(card.quants["Q4_K_M"].context_length, Some(8192));

    // Assert model config entry was added. Key is now derived from the
    // bare repo slug (no per-quant suffix), so all quants of the same
    // repo share one model entry.
    let model_key = repo_slug.to_lowercase();
    assert!(
        config.models.contains_key(&model_key),
        "Expected model key '{}' in config, got: {:?}",
        model_key,
        config.models.keys().collect::<Vec<_>>()
    );
    // Verify the entry's `model` field points to the original repo (this
    // is what the dedupe-by-model logic uses).
    assert_eq!(config.models[&model_key].model.as_deref(), Some(repo_id));
}

/// Verifies that `PullJob` serializes to JSON with the fields expected for SSE data.
#[test]
fn test_pull_job_serializes_for_sse() {
    let job = PullJob {
        job_id: "pull-test-123".to_string(),
        repo_id: "bartowski/Qwen3-8B-GGUF".to_string(),
        filename: "Qwen3-8B-Q4_K_M.gguf".to_string(),
        status: PullJobStatus::Running,
        bytes_downloaded: 1_234_567,
        total_bytes: Some(4_800_000_000),
        ..Default::default()
    };

    let json = serde_json::to_string(&job).expect("PullJob serialization failed");
    assert!(
        json.contains("\"bytes_downloaded\""),
        "missing bytes_downloaded in: {json}"
    );
    assert!(json.contains("\"status\""), "missing status in: {json}");
    assert!(
        json.contains("\"running\""),
        "missing running status value in: {json}"
    );
    assert!(json.contains("\"job_id\""), "missing job_id in: {json}");
    // New verification fields must be present in the SSE payload so the
    // wizard can render the verify-phase progress bar.
    assert!(
        json.contains("\"verify_bytes_hashed\""),
        "missing verify_bytes_hashed in: {json}"
    );
    assert!(
        json.contains("\"verify_total_bytes\""),
        "missing verify_total_bytes in: {json}"
    );
    assert!(
        json.contains("\"verified_ok\""),
        "missing verified_ok in: {json}"
    );
    assert!(
        json.contains("\"verify_error\""),
        "missing verify_error in: {json}"
    );
}

/// Verifies that `PullJobStatus::Verifying` serializes as the snake_case
/// string `"verifying"` so frontends can match on it.
#[test]
fn test_pull_job_status_verifying_serializes() {
    let job = PullJob {
        status: PullJobStatus::Verifying,
        ..Default::default()
    };
    let json = serde_json::to_string(&job).unwrap();
    assert!(
        json.contains("\"status\":\"verifying\""),
        "expected verifying status string in: {json}"
    );
}

/// Verifies that `QuantEntry` serializes to JSON with all expected keys.
#[test]
fn test_quant_entry_serializes() {
    let entry = QuantEntry {
        filename: "Model-Q4_K_M.gguf".to_string(),
        quant: Some("Q4_K_M".to_string()),
        size_bytes: Some(4_200_000_000),
        kind: crate::config::QuantKind::Model,
    };

    let value = serde_json::to_value(&entry).expect("serialization failed");
    assert!(value.get("filename").is_some(), "missing filename");
    assert!(value.get("quant").is_some(), "missing quant");
    assert!(value.get("size_bytes").is_some(), "missing size_bytes");
    assert!(value.get("kind").is_some(), "missing kind");
    assert_eq!(value["filename"], "Model-Q4_K_M.gguf");
    assert_eq!(value["quant"], "Q4_K_M");
    assert_eq!(value["size_bytes"], 4_200_000_000_i64);
    assert_eq!(value["kind"], "model");
}

/// Verifies that `SystemHealthResponse` serializes to JSON with all expected fields.
#[test]
fn test_system_health_response_serializes() {
    let response = super::system::SystemHealthResponse {
        status: "ok",
        service: "koji",
        models_loaded: 2,
        cpu_usage_pct: 42.5,
        ram_used_mib: 1024,
        ram_total_mib: 8192,
        gpu_utilization_pct: Some(75),
        vram: Some(crate::gpu::VramInfo {
            used_mib: 4000,
            total_mib: 8000,
        }),
    };

    let value = serde_json::to_value(&response).expect("serialization failed");
    assert!(
        value.get("cpu_usage_pct").is_some(),
        "missing cpu_usage_pct"
    );
    assert!(value.get("ram_used_mib").is_some(), "missing ram_used_mib");
    assert!(
        value.get("ram_total_mib").is_some(),
        "missing ram_total_mib"
    );
    assert!(
        value.get("gpu_utilization_pct").is_some(),
        "missing gpu_utilization_pct"
    );
    assert!(value.get("vram").is_some(), "missing vram");
}
