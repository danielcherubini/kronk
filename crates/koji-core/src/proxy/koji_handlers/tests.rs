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
    let config = crate::config::Config {
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
    let mut models = std::collections::HashMap::new();
    _setup_model_after_pull_with_config(&configs_dir, &mut models, repo_id, &spec, &dest_dir).await;

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
        models.contains_key(&model_key),
        "Expected model key '{}' in models map, got: {:?}",
        model_key,
        models.keys().collect::<Vec<_>>()
    );
    // Verify the entry's `model` field points to the original repo (this
    // is what the dedupe-by-model logic uses).
    assert_eq!(models[&model_key].model.as_deref(), Some(repo_id));
}

/// Pulling an mmproj file AFTER the parent quant exists should auto-enable
/// vision on the parent: `mmproj` set to the filename and "image" added to
/// input modalities. Locks in the auto-enable behavior (commit 461459f).
#[tokio::test]
async fn test_mmproj_pull_auto_enables_vision_on_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(config_dir.join("configs")).unwrap();
    let repo_id = "bartowski/TestVision-GGUF";
    let dest_dir = config_dir.join("models").join(repo_id);
    std::fs::create_dir_all(&dest_dir).unwrap();

    let configs_dir = config_dir.join("configs");
    std::fs::create_dir_all(&configs_dir).unwrap();
    let config = crate::config::Config {
        loaded_from: Some(config_dir.clone()),
        ..Default::default()
    };
    config.save_to(&config_dir).unwrap();

    // Pull 1: parent quant.
    let parent_spec = super::types::QuantDownloadSpec {
        filename: "TestVision-Q4_K_M.gguf".to_string(),
        quant: Some("Q4_K_M".to_string()),
        context_length: None,
    };
    std::fs::write(dest_dir.join(&parent_spec.filename), b"x").unwrap();
    let mut models = std::collections::HashMap::new();
    _setup_model_after_pull_with_config(
        &configs_dir,
        &mut models,
        repo_id,
        &parent_spec,
        &dest_dir,
    )
    .await;

    let key = repo_id.replace('/', "--").to_lowercase();
    assert!(models[&key].mmproj.is_none(), "mmproj should start unset");

    // Pull 2: mmproj sibling.
    let mmproj_spec = super::types::QuantDownloadSpec {
        filename: "mmproj-TestVision-f16.gguf".to_string(),
        quant: None,
        context_length: None,
    };
    std::fs::write(dest_dir.join(&mmproj_spec.filename), b"x").unwrap();
    let returned_key = _setup_model_after_pull_with_config(
        &configs_dir,
        &mut models,
        repo_id,
        &mmproj_spec,
        &dest_dir,
    )
    .await;

    assert_eq!(
        returned_key.as_deref(),
        Some(key.as_str()),
        "mmproj path must return the parent's key so setup_model_after_pull saves it"
    );
    let mc = &models[&key];
    assert_eq!(mc.mmproj.as_deref(), Some("mmproj-TestVision-f16.gguf"));
    let modalities = mc.modalities.as_ref().expect("modalities set");
    assert!(
        modalities
            .input
            .iter()
            .any(|m| m.eq_ignore_ascii_case("image")),
        "input modalities should include 'image', got {:?}",
        modalities.input
    );
}

/// Pulling an mmproj file BEFORE any parent quant should create a disabled stub
/// entry. A subsequent main-quant pull must promote that stub: set `quant`,
/// flip `enabled` to true, and retain the mmproj wiring from the first pull.
#[tokio::test]
async fn test_mmproj_pull_before_parent_creates_stub_then_promotes() {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().to_path_buf();
    let configs_dir = config_dir.join("configs");
    std::fs::create_dir_all(&configs_dir).unwrap();
    let repo_id = "bartowski/TestVisionEarly-GGUF";
    let dest_dir = config_dir.join("models").join(repo_id);
    std::fs::create_dir_all(&dest_dir).unwrap();

    let config = crate::config::Config {
        loaded_from: Some(config_dir.clone()),
        ..Default::default()
    };
    config.save_to(&config_dir).unwrap();

    // Pull 1: mmproj first, no parent exists yet.
    let mmproj_spec = super::types::QuantDownloadSpec {
        filename: "mmproj-TestVisionEarly-f16.gguf".to_string(),
        quant: None,
        context_length: None,
    };
    std::fs::write(dest_dir.join(&mmproj_spec.filename), b"x").unwrap();
    let mut models = std::collections::HashMap::new();
    _setup_model_after_pull_with_config(
        &configs_dir,
        &mut models,
        repo_id,
        &mmproj_spec,
        &dest_dir,
    )
    .await;

    let key = repo_id.replace('/', "--").to_lowercase();
    let stub = &models[&key];
    assert_eq!(stub.quant, None, "stub should start without quant");
    assert!(!stub.enabled, "stub should start disabled");
    assert_eq!(
        stub.mmproj.as_deref(),
        Some("mmproj-TestVisionEarly-f16.gguf")
    );

    // Pull 2: main quant arrives later, must promote the stub.
    let parent_spec = super::types::QuantDownloadSpec {
        filename: "TestVisionEarly-Q4_K_M.gguf".to_string(),
        quant: Some("Q4_K_M".to_string()),
        context_length: Some(4096),
    };
    std::fs::write(dest_dir.join(&parent_spec.filename), b"x").unwrap();
    _setup_model_after_pull_with_config(
        &configs_dir,
        &mut models,
        repo_id,
        &parent_spec,
        &dest_dir,
    )
    .await;

    let promoted = &models[&key];
    assert_eq!(
        promoted.quant.as_deref(),
        Some("Q4_K_M"),
        "main-quant pull must fill in quant"
    );
    assert!(
        promoted.enabled,
        "main-quant pull must flip enabled to true"
    );
    assert_eq!(
        promoted.context_length,
        Some(4096),
        "main-quant pull must fill in context_length"
    );
    assert_eq!(
        promoted.mmproj.as_deref(),
        Some("mmproj-TestVisionEarly-f16.gguf"),
        "mmproj wiring from the first pull must survive promotion"
    );
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
