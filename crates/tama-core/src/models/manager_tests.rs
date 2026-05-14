//! Tests for ModelManager.

use super::*;
use crate::config::ModelConfig;
use crate::db::queries::{DownloadLogEntry, ModelConfigRecord, UpdateCheckParams};

fn make_test_record(repo_id: &str) -> ModelConfigRecord {
    use chrono::{SecondsFormat, Utc};
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    ModelConfigRecord {
        id: 0,
        repo_id: repo_id.to_string(),
        display_name: Some("Test Model".to_string()),
        backend: "llama.cpp".to_string(),
        gpu_variant: None,
        enabled: true,
        selected_quant: None,
        selected_mmproj: None,
        context_length: None,
        num_parallel: None,
        kv_unified: false,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        port: None,
        args: None,
        sampling: None,
        modalities: None,
        profile: None,
        api_name: Some(repo_id.to_string()),
        health_check: None,
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        created_at: now.clone(),
        updated_at: now,
    }
}

#[test]
fn test_open_in_memory() {
    let manager = ModelManager::open_in_memory().unwrap();
    let _conn = manager.conn();
    let configs = manager.get_all_configs().unwrap();
    assert!(configs.is_empty());
}

#[test]
fn test_upsert_and_get_config() {
    let manager = ModelManager::open_in_memory().unwrap();
    let record = make_test_record("owner/test-repo");
    let id = manager.upsert_config(&record).unwrap();
    assert_eq!(id, 1);

    let fetched = manager.get_config(id).unwrap().unwrap();
    assert_eq!(fetched.repo_id, "owner/test-repo");
    assert_eq!(fetched.display_name, Some("Test Model".to_string()));

    let all = manager.get_all_configs().unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn test_get_config_by_repo_id_missing() {
    let manager = ModelManager::open_in_memory().unwrap();
    let result = manager.get_config_by_repo_id("nonexistent/repo").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_enable_disable_model() {
    let manager = ModelManager::open_in_memory().unwrap();

    let mc = ModelConfig {
        backend: "llama.cpp".to_string(),
        enabled: true,
        ..Default::default()
    };
    manager.save_model_config("owner--test-repo", &mc).unwrap();

    // Disable it
    manager.disable_model("owner--test-repo").unwrap();
    let record = manager
        .get_config_by_repo_id("owner/test-repo")
        .unwrap()
        .unwrap();
    assert!(!record.enabled);

    // Re-enable it
    manager.enable_model("owner--test-repo").unwrap();
    let record = manager
        .get_config_by_repo_id("owner/test-repo")
        .unwrap()
        .unwrap();
    assert!(record.enabled);
}

#[test]
fn test_rename_config() {
    let manager = ModelManager::open_in_memory().unwrap();
    let record = make_test_record("owner/old-name");
    let id = manager.upsert_config(&record).unwrap();

    manager.rename_config(id, "owner/new-name").unwrap();

    // Old repo_id should return None
    let old = manager.get_config_by_repo_id("owner/old-name").unwrap();
    assert!(old.is_none());

    // New repo_id should return the record
    let new = manager
        .get_config_by_repo_id("owner/new-name")
        .unwrap()
        .unwrap();
    assert_eq!(new.repo_id, "owner/new-name");
    assert_eq!(new.display_name, Some("Test Model".to_string()));
}

#[test]
fn test_file_operations() {
    let manager = ModelManager::open_in_memory().unwrap();

    // Insert a model config first (required for FK)
    let record = make_test_record("owner/test-model");
    let model_id = manager.upsert_config(&record).unwrap();

    // Verify no files initially
    let files = manager.get_files(model_id).unwrap();
    assert!(files.is_empty());

    // Upsert a file
    manager
        .upsert_file(
            model_id,
            "owner/test-model",
            "test-model.Q4_K_M.gguf",
            Some("Q4_K_M"),
            Some("sha256-abc123"),
            Some(1_000_000),
        )
        .unwrap();

    // Verify it appears in get_files
    let files = manager.get_files(model_id).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename, "test-model.Q4_K_M.gguf");
    assert_eq!(files[0].quant, Some("Q4_K_M".to_string()));
    assert_eq!(files[0].lfs_oid, Some("sha256-abc123".to_string()));
    assert_eq!(files[0].size_bytes, Some(1_000_000));

    // Verify it appears in get_all_files
    let all_files = manager.get_all_files().unwrap();
    assert_eq!(all_files.len(), 1);

    // Update verification
    manager
        .update_verification(model_id, "test-model.Q4_K_M.gguf", Some(true), None)
        .unwrap();

    let files = manager.get_files(model_id).unwrap();
    assert_eq!(files[0].verified_ok, Some(true));

    // Delete the file
    manager
        .delete_file(model_id, "test-model.Q4_K_M.gguf")
        .unwrap();

    let files = manager.get_files(model_id).unwrap();
    assert!(files.is_empty());
}

#[test]
fn test_pull_operations() {
    let manager = ModelManager::open_in_memory().unwrap();

    // Insert a model config
    let record = make_test_record("owner/test-model");
    let model_id = manager.upsert_config(&record).unwrap();

    // No pull record initially
    let pull = manager.get_pull(model_id).unwrap();
    assert!(pull.is_none());

    // Upsert a pull record
    manager
        .upsert_pull(model_id, "owner/test-model", "abc123def456")
        .unwrap();

    // Verify pull record
    let pull = manager.get_pull(model_id).unwrap().unwrap();
    assert_eq!(pull.model_id, model_id);
    assert_eq!(pull.repo_id, "owner/test-model");
    assert_eq!(pull.commit_sha, "abc123def456");
}

#[test]
fn test_log_download() {
    let manager = ModelManager::open_in_memory().unwrap();

    let entry = DownloadLogEntry {
        repo_id: "owner/test-model".to_string(),
        filename: "test.gguf".to_string(),
        started_at: "2025-01-01T00:00:00Z".to_string(),
        completed_at: Some("2025-01-01T00:01:00Z".to_string()),
        size_bytes: Some(5_000_000),
        duration_ms: Some(60_000),
        success: true,
        error_message: None,
    };

    manager.log_download(&entry).unwrap();
}

#[test]
fn test_active_model_operations() {
    let manager = ModelManager::open_in_memory().unwrap();

    // Initially empty
    let active = manager.get_active().unwrap();
    assert!(active.is_empty());

    // Insert an active record
    manager
        .insert_active(
            "server1",
            "model.gguf",
            "llama.cpp",
            1234,
            8080,
            "http://127.0.0.1:8080",
        )
        .unwrap();

    // Verify it appears
    let active = manager.get_active().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].server_name, "server1");
    assert_eq!(active[0].model_name, "model.gguf");
    assert_eq!(active[0].backend, "llama.cpp");
    assert_eq!(active[0].pid, 1234);
    assert_eq!(active[0].port, 8080);

    // Rename the active record
    manager.rename_active("server1", "server1-renamed").unwrap();
    let active = manager.get_active().unwrap();
    assert_eq!(active[0].server_name, "server1-renamed");

    // Remove the active record
    manager.remove_active("server1-renamed").unwrap();
    let active = manager.get_active().unwrap();
    assert!(active.is_empty());
}

#[test]
fn test_download_queue_operations() {
    let manager = ModelManager::open_in_memory().unwrap();

    // Insert a queue item
    let id = manager
        .queue_insert(
            "pull-abc123",
            "owner/test-model",
            "test-model.Q4_K_M.gguf",
            Some("Test Model Q4"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();
    assert!(id > 0);

    // Get queued item
    let item = manager.queue_get_queued().unwrap().unwrap();
    assert_eq!(item.job_id, "pull-abc123");
    assert_eq!(item.status, "queued");
    assert_eq!(item.kind, "model");
    assert_eq!(item.quant, Some("Q4_K_M".to_string()));
    assert_eq!(item.context_length, Some(4096));

    // Update status to running
    manager
        .queue_update_status("pull-abc123", "running", 500, Some(1000), None)
        .unwrap();

    // Get by job_id
    let item = manager.queue_get_by_job_id("pull-abc123").unwrap().unwrap();
    assert_eq!(item.status, "running");
    assert_eq!(item.bytes_downloaded, 500);

    // Get active items
    let active = manager.queue_get_active().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].job_id, "pull-abc123");

    // Complete the item
    manager
        .queue_update_status("pull-abc123", "completed", 1000, Some(1000), None)
        .unwrap();

    // Should appear in history now
    let history = manager.queue_get_history(10, 0).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, "completed");

    // Should no longer be in active
    let active = manager.queue_get_active().unwrap();
    assert!(active.is_empty());
}

#[test]
fn test_queue_cancel() {
    let manager = ModelManager::open_in_memory().unwrap();

    manager
        .queue_insert(
            "pull-cancel1",
            "owner/test",
            "test.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();

    manager.queue_cancel("pull-cancel1").unwrap();

    let item = manager
        .queue_get_by_job_id("pull-cancel1")
        .unwrap()
        .unwrap();
    assert_eq!(item.status, "cancelled");
    assert!(item.completed_at.is_some());
}

#[test]
fn test_update_check_operations() {
    let manager = ModelManager::open_in_memory().unwrap();

    // Initially no update check
    let check = manager.get_update_check("backend", "llama.cpp").unwrap();
    assert!(check.is_none());

    // Upsert an update check
    let params = UpdateCheckParams {
        item_type: "backend",
        item_id: "llama.cpp",
        current_version: Some("0.1"),
        latest_version: Some("0.2"),
        update_available: true,
        status: "update_available",
        error_message: None,
        details_json: None,
        checked_at: 1700000000,
    };
    manager.upsert_update_check(params).unwrap();

    // Retrieve it
    let check = manager
        .get_update_check("backend", "llama.cpp")
        .unwrap()
        .unwrap();
    assert_eq!(check.item_type, "backend");
    assert_eq!(check.item_id, "llama.cpp");
    assert_eq!(check.current_version, Some("0.1".to_string()));
    assert_eq!(check.latest_version, Some("0.2".to_string()));
    assert!(check.update_available);

    // Delete it
    manager.delete_update_check("backend", "llama.cpp").unwrap();
    let check = manager.get_update_check("backend", "llama.cpp").unwrap();
    assert!(check.is_none());
}

#[test]
fn test_save_model_config_convenience() {
    let manager = ModelManager::open_in_memory().unwrap();

    let mc = ModelConfig {
        backend: "llama.cpp".to_string(),
        display_name: Some("My Model".to_string()),
        enabled: true,
        ..Default::default()
    };
    let id = manager.save_model_config("owner--my-model", &mc).unwrap();
    assert_eq!(id, 1);

    let record = manager.get_config(id).unwrap().unwrap();
    assert_eq!(record.repo_id, "owner/my-model");
    assert_eq!(record.backend, "llama.cpp");
    assert_eq!(record.display_name, Some("My Model".to_string()));
    assert!(record.enabled);
    assert_eq!(record.api_name, Some("owner/my-model".to_string()));
}
