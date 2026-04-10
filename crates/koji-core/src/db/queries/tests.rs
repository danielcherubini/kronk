//! Tests for database query functions.

use crate::db::{open_in_memory, OpenResult};

use super::*;

/// Tests upserting and retrieving a model pull record
#[test]
fn test_upsert_and_get_model_pull() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    // Insert
    upsert_model_pull(&conn, "bartowski/OmniCoder-8B-GGUF", "abc123").unwrap();
    let record = get_model_pull(&conn, "bartowski/OmniCoder-8B-GGUF")
        .unwrap()
        .unwrap();
    assert_eq!(record.repo_id, "bartowski/OmniCoder-8B-GGUF");
    assert_eq!(record.commit_sha, "abc123");
    assert!(!record.pulled_at.is_empty());

    // Update with new SHA
    upsert_model_pull(&conn, "bartowski/OmniCoder-8B-GGUF", "def456").unwrap();
    let updated = get_model_pull(&conn, "bartowski/OmniCoder-8B-GGUF")
        .unwrap()
        .unwrap();
    assert_eq!(updated.commit_sha, "def456");
}

/// Tests upserting and retrieving model files
#[test]
fn test_upsert_and_get_model_files() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let repo = "bartowski/OmniCoder-8B-GGUF";

    // Insert two files
    upsert_model_file(
        &conn,
        repo,
        "Model-Q4_K_M.gguf",
        Some("Q4_K_M"),
        Some("sha256_a"),
        Some(4_200_000_000),
    )
    .unwrap();
    upsert_model_file(
        &conn,
        repo,
        "Model-Q8_0.gguf",
        Some("Q8_0"),
        Some("sha256_b"),
        Some(8_400_000_000),
    )
    .unwrap();

    let files = get_model_files(&conn, repo).unwrap();
    assert_eq!(files.len(), 2);

    // Update one file's lfs_oid
    upsert_model_file(
        &conn,
        repo,
        "Model-Q4_K_M.gguf",
        Some("Q4_K_M"),
        Some("sha256_new"),
        Some(4_300_000_000),
    )
    .unwrap();
    let files2 = get_model_files(&conn, repo).unwrap();
    assert_eq!(files2.len(), 2);
    let updated = files2
        .iter()
        .find(|f| f.filename == "Model-Q4_K_M.gguf")
        .unwrap();
    assert_eq!(updated.lfs_oid.as_deref(), Some("sha256_new"));
    assert_eq!(updated.size_bytes, Some(4_300_000_000));
}

#[test]
fn test_upsert_preserves_verification_when_hash_unchanged() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let repo = "test/repo";

    // Initial insert with a hash
    upsert_model_file(
        &conn,
        repo,
        "Model-Q4_K_M.gguf",
        Some("Q4_K_M"),
        Some("sha_abc"),
        Some(1000),
    )
    .unwrap();

    // Mark it as verified
    update_verification(&conn, repo, "Model-Q4_K_M.gguf", Some(true), None).unwrap();

    // Re-upsert with the SAME hash (e.g. refresh_metadata from HF)
    upsert_model_file(
        &conn,
        repo,
        "Model-Q4_K_M.gguf",
        Some("Q4_K_M"),
        Some("sha_abc"),
        Some(1000),
    )
    .unwrap();

    let files = get_model_files(&conn, repo).unwrap();
    let file = files
        .iter()
        .find(|f| f.filename == "Model-Q4_K_M.gguf")
        .unwrap();
    assert_eq!(
        file.verified_ok,
        Some(true),
        "verification state should be preserved when lfs_oid is unchanged"
    );
    assert!(file.last_verified_at.is_some());
}

#[test]
fn test_upsert_clears_verification_when_hash_changes() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let repo = "test/repo";

    upsert_model_file(
        &conn,
        repo,
        "Model-Q4_K_M.gguf",
        Some("Q4_K_M"),
        Some("sha_old"),
        Some(1000),
    )
    .unwrap();
    update_verification(&conn, repo, "Model-Q4_K_M.gguf", Some(true), None).unwrap();

    // Re-upsert with a DIFFERENT hash (file was updated on HF)
    upsert_model_file(
        &conn,
        repo,
        "Model-Q4_K_M.gguf",
        Some("Q4_K_M"),
        Some("sha_new"),
        Some(1100),
    )
    .unwrap();

    let files = get_model_files(&conn, repo).unwrap();
    let file = files
        .iter()
        .find(|f| f.filename == "Model-Q4_K_M.gguf")
        .unwrap();
    assert_eq!(
        file.verified_ok, None,
        "verification state should be cleared when lfs_oid changes"
    );
    assert!(file.last_verified_at.is_none());
    assert!(file.verify_error.is_none());
}

#[test]
fn test_update_verification_writes_error() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let repo = "test/repo";

    upsert_model_file(&conn, repo, "x.gguf", None, Some("sha"), Some(1)).unwrap();
    update_verification(
        &conn,
        repo,
        "x.gguf",
        Some(false),
        Some("hash mismatch: expected ab got cd"),
    )
    .unwrap();

    let files = get_model_files(&conn, repo).unwrap();
    let f = &files[0];
    assert_eq!(f.verified_ok, Some(false));
    assert_eq!(
        f.verify_error.as_deref(),
        Some("hash mismatch: expected ab got cd")
    );
}

#[test]
fn test_log_download() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let entry = DownloadLogEntry {
        repo_id: "bartowski/OmniCoder-8B-GGUF".to_string(),
        filename: "Model-Q4_K_M.gguf".to_string(),
        started_at: "2024-01-01T00:00:00.000Z".to_string(),
        completed_at: Some("2024-01-01T00:01:00.000Z".to_string()),
        size_bytes: Some(4_200_000_000),
        duration_ms: Some(60_000),
        success: true,
        error_message: None,
    };
    log_download(&conn, &entry).unwrap();

    // Query it back
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM download_log", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);

    let (repo_id, success): (String, i64) = conn
        .query_row(
            "SELECT repo_id, success FROM download_log LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(repo_id, "bartowski/OmniCoder-8B-GGUF");
    assert_eq!(success, 1);
}

#[test]
fn test_get_model_pull_not_found() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let result = get_model_pull(&conn, "unknown/repo").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_get_model_files_empty() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let files = get_model_files(&conn, "unknown/repo").unwrap();
    assert!(files.is_empty());
}

#[test]
fn test_delete_model_records() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();
    let repo = "bartowski/OmniCoder-8B-GGUF";

    // Insert records
    upsert_model_pull(&conn, repo, "abc123").unwrap();
    upsert_model_file(&conn, repo, "Model-Q4_K_M.gguf", Some("Q4_K_M"), None, None).unwrap();

    // Also insert a download log entry
    log_download(
        &conn,
        &DownloadLogEntry {
            repo_id: repo.to_string(),
            filename: "Model-Q4_K_M.gguf".to_string(),
            started_at: "2024-01-01T00:00:00.000Z".to_string(),
            completed_at: None,
            size_bytes: None,
            duration_ms: None,
            success: false,
            error_message: Some("test".to_string()),
        },
    )
    .unwrap();

    // Delete
    delete_model_records(&conn, repo).unwrap();

    // Verify pulls and files are gone
    assert!(get_model_pull(&conn, repo).unwrap().is_none());
    assert!(get_model_files(&conn, repo).unwrap().is_empty());

    // Verify download_log is preserved
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM download_log", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_migration_v2_creates_active_models() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    // Verify active_models table exists
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='active_models'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_needs_backfill_true_on_fresh_db() {
    let result = open_in_memory().unwrap();
    assert!(result.needs_backfill);
}

#[test]
fn test_needs_backfill_false_on_existing_db() {
    // Use a real file so the DB persists between two opens
    let tmp = tempfile::tempdir().unwrap();
    let first = crate::db::open(tmp.path()).unwrap();
    assert!(first.needs_backfill, "first open should need backfill");
    drop(first.conn);

    let second = crate::db::open(tmp.path()).unwrap();
    assert!(
        !second.needs_backfill,
        "second open should not need backfill"
    );
}

#[test]
fn test_insert_and_get_active_models() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    insert_active_model(
        &conn,
        "test-server",
        "test-model",
        "llama-server",
        12345,
        8080,
        "http://127.0.0.1:8080",
    )
    .unwrap();

    let models = get_active_models(&conn).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].server_name, "test-server");
    assert_eq!(models[0].model_name, "test-model");
    assert_eq!(models[0].backend, "llama-server");
    assert_eq!(models[0].pid, 12345);
    assert_eq!(models[0].port, 8080);
    assert_eq!(models[0].backend_url, "http://127.0.0.1:8080");
}

#[test]
fn test_remove_active_model() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    insert_active_model(
        &conn,
        "test-server",
        "test-model",
        "llama-server",
        12345,
        8080,
        "http://127.0.0.1:8080",
    )
    .unwrap();

    assert_eq!(get_active_models(&conn).unwrap().len(), 1);

    remove_active_model(&conn, "test-server").unwrap();

    assert!(get_active_models(&conn).unwrap().is_empty());
}

#[test]
fn test_clear_active_models() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    insert_active_model(
        &conn,
        "server1",
        "model1",
        "llama-server",
        1001,
        8001,
        "http://127.0.0.1:8001",
    )
    .unwrap();
    insert_active_model(
        &conn,
        "server2",
        "model2",
        "llama-server",
        1002,
        8002,
        "http://127.0.0.1:8002",
    )
    .unwrap();

    assert_eq!(get_active_models(&conn).unwrap().len(), 2);

    clear_active_models(&conn).unwrap();

    assert!(get_active_models(&conn).unwrap().is_empty());
}

#[test]
fn test_touch_active_model() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    insert_active_model(
        &conn,
        "test-server",
        "test-model",
        "llama-server",
        12345,
        8080,
        "http://127.0.0.1:8080",
    )
    .unwrap();

    let models = get_active_models(&conn).unwrap();
    let loaded_at1 = models[0].loaded_at.clone();

    // Wait a bit to ensure different timestamp
    std::thread::sleep(std::time::Duration::from_millis(200));

    touch_active_model(&conn, "test-server").unwrap();

    let models = get_active_models(&conn).unwrap();
    assert_ne!(models[0].last_accessed, loaded_at1);
}

#[test]
fn test_rename_active_model() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    // Insert an active model with old name
    insert_active_model(
        &conn,
        "old-name",
        "test-model",
        "llama-server",
        12345,
        8080,
        "http://127.0.0.1:8080",
    )
    .unwrap();

    // Rename
    rename_active_model(&conn, "old-name", "new-name").unwrap();

    // Verify old name is gone and new name exists
    let models = get_active_models(&conn).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].server_name, "new-name");

    // Verify old name is gone
    let old_model = conn
        .query_row(
            "SELECT COUNT(*) FROM active_models WHERE server_name = ?",
            ["old-name"],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(old_model, 0);
}

#[test]
fn test_rename_active_model_not_found() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    // Rename a name that doesn't exist — should succeed (0 rows affected is OK)
    let result = rename_active_model(&conn, "non-existent", "new-name");
    assert!(result.is_ok());
}

// -----------------------------------------------------------------------
// backend_installations tests
// -----------------------------------------------------------------------

fn _make_record(name: &str, version: &str, installed_at: i64) -> BackendInstallationRecord {
    BackendInstallationRecord {
        id: 0,
        name: name.to_string(),
        backend_type: "llama_cpp".to_string(),
        version: version.to_string(),
        path: format!("/opt/backends/{name}/{version}"),
        installed_at,
        gpu_type: None,
        source: None,
        is_active: false,
    }
}

#[test]
fn test_insert_and_get_active_backend() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let r1 = _make_record("llama_cpp", "v1.0.0", 1000);
    insert_backend_installation(&conn, &r1).unwrap();

    let r2 = _make_record("llama_cpp", "v2.0.0", 2000);
    insert_backend_installation(&conn, &r2).unwrap();

    let active = get_active_backend(&conn, "llama_cpp").unwrap().unwrap();
    assert_eq!(active.version, "v2.0.0");
    assert!(active.is_active);
}

#[test]
fn test_list_active_backends() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let r1 = _make_record("llama_cpp", "v1.0.0", 1000);
    insert_backend_installation(&conn, &r1).unwrap();

    let r2 = _make_record("ik_llama", "v1.0.0", 1000);
    insert_backend_installation(&conn, &r2).unwrap();

    let active = list_active_backends(&conn).unwrap();
    assert_eq!(active.len(), 2);
}

#[test]
fn test_list_backend_versions() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let r1 = _make_record("llama_cpp", "v1.0.0", 1000);
    insert_backend_installation(&conn, &r1).unwrap();

    let r2 = _make_record("llama_cpp", "v2.0.0", 2000);
    insert_backend_installation(&conn, &r2).unwrap();

    let versions = list_backend_versions(&conn, "llama_cpp").unwrap();
    assert_eq!(versions.len(), 2);
    // Ordered newest first (installed_at DESC)
    assert_eq!(versions[0].version, "v2.0.0");
    assert_eq!(versions[1].version, "v1.0.0");
}

#[test]
fn test_delete_single_backend_version() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let r1 = _make_record("llama_cpp", "v1.0.0", 1000);
    insert_backend_installation(&conn, &r1).unwrap();

    let r2 = _make_record("llama_cpp", "v2.0.0", 2000);
    insert_backend_installation(&conn, &r2).unwrap();

    delete_backend_installation(&conn, "llama_cpp", "v1.0.0").unwrap();

    let versions = list_backend_versions(&conn, "llama_cpp").unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].version, "v2.0.0");
}

#[test]
fn test_delete_all_backend_versions() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let r1 = _make_record("llama_cpp", "v1.0.0", 1000);
    insert_backend_installation(&conn, &r1).unwrap();

    let r2 = _make_record("llama_cpp", "v2.0.0", 2000);
    insert_backend_installation(&conn, &r2).unwrap();

    delete_all_backend_versions(&conn, "llama_cpp").unwrap();

    let versions = list_backend_versions(&conn, "llama_cpp").unwrap();
    assert!(versions.is_empty());
}

#[test]
fn test_get_backend_by_version() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    // Insert two versions of llama_cpp with distinct paths
    let r1 = BackendInstallationRecord {
        id: 0,
        name: "llama_cpp".to_string(),
        backend_type: "llama_cpp".to_string(),
        version: "v1.0.0".to_string(),
        path: "/v1/llama-server".to_string(),
        installed_at: 1000,
        gpu_type: None,
        source: None,
        is_active: false,
    };
    insert_backend_installation(&conn, &r1).unwrap();

    let r2 = BackendInstallationRecord {
        id: 0,
        name: "llama_cpp".to_string(),
        backend_type: "llama_cpp".to_string(),
        version: "v2.0.0".to_string(),
        path: "/v2/llama-server".to_string(),
        installed_at: 2000,
        gpu_type: None,
        source: None,
        is_active: false,
    };
    insert_backend_installation(&conn, &r2).unwrap();

    // v1.0.0 should be found with path /v1/llama-server
    let found = get_backend_by_version(&conn, "llama_cpp", "v1.0.0")
        .unwrap()
        .unwrap();
    assert_eq!(found.path, "/v1/llama-server");
    assert_eq!(found.version, "v1.0.0");

    // v2.0.0 should be found with path /v2/llama-server
    let found = get_backend_by_version(&conn, "llama_cpp", "v2.0.0")
        .unwrap()
        .unwrap();
    assert_eq!(found.path, "/v2/llama-server");
    assert_eq!(found.version, "v2.0.0");

    // unknown version should return Ok(None)
    let not_found = get_backend_by_version(&conn, "llama_cpp", "unknown").unwrap();
    assert!(not_found.is_none());
}

#[test]
fn test_insert_same_version_is_idempotent() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let r1 = _make_record("llama_cpp", "b8407", 1000);
    insert_backend_installation(&conn, &r1).unwrap();

    // Same (name, version) — should succeed (INSERT OR REPLACE), not error
    let r2 = _make_record("llama_cpp", "b8407", 2000);
    let result = insert_backend_installation(&conn, &r2);
    assert!(
        result.is_ok(),
        "reinstalling the same (name, version) should be idempotent, got: {:?}",
        result
    );

    // Only one row should exist for this (name, version)
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM backend_installations WHERE name = 'llama_cpp' AND version = 'b8407'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "only one row should exist after idempotent insert"
    );
}

// -----------------------------------------------------------------------
// system_metrics_history tests
// -----------------------------------------------------------------------

#[test]
fn test_insert_and_get_recent_system_metrics() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let row1 = SystemMetricsRow {
        ts_unix_ms: 1_000_000_000_000,
        cpu_usage_pct: 25.5,
        ram_used_mib: 2048,
        ram_total_mib: 8192,
        gpu_utilization_pct: Some(45),
        vram_used_mib: Some(1024),
        vram_total_mib: Some(8192),
        models_loaded: 3,
    };
    insert_system_metric(&conn, &row1, 0).unwrap();

    let row2 = SystemMetricsRow {
        ts_unix_ms: 2_000_000_000_000,
        cpu_usage_pct: 50.0,
        ram_used_mib: 4096,
        ram_total_mib: 8192,
        gpu_utilization_pct: Some(70),
        vram_used_mib: Some(4096),
        vram_total_mib: Some(8192),
        models_loaded: 5,
    };
    insert_system_metric(&conn, &row2, 0).unwrap();

    let recent = get_recent_system_metrics(&conn, 10).unwrap();
    assert_eq!(recent.len(), 2);
    // Should return oldest-first
    assert_eq!(recent[0].ts_unix_ms, 1_000_000_000_000);
    assert_eq!(recent[1].ts_unix_ms, 2_000_000_000_000);
}

#[test]
fn test_insert_system_metric_prunes_old_rows() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let old_row = SystemMetricsRow {
        ts_unix_ms: 1_000_000_000_000,
        cpu_usage_pct: 10.0,
        ram_used_mib: 1024,
        ram_total_mib: 8192,
        gpu_utilization_pct: None,
        vram_used_mib: None,
        vram_total_mib: None,
        models_loaded: 0,
    };
    insert_system_metric(&conn, &old_row, 5_000_000_000_000).unwrap();

    let new_row = SystemMetricsRow {
        ts_unix_ms: 6_000_000_000_000,
        cpu_usage_pct: 30.0,
        ram_used_mib: 3072,
        ram_total_mib: 8192,
        gpu_utilization_pct: Some(25),
        vram_used_mib: Some(512),
        vram_total_mib: Some(8192),
        models_loaded: 1,
    };
    insert_system_metric(&conn, &new_row, 5_000_000_000_000).unwrap();

    // old_row should have been pruned (ts_unix_ms < cutoff_ms)
    let recent = get_recent_system_metrics(&conn, 10).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].ts_unix_ms, 6_000_000_000_000);
}

#[test]
fn test_get_system_metrics_since() {
    let OpenResult { conn, .. } = open_in_memory().unwrap();

    let row1 = SystemMetricsRow {
        ts_unix_ms: 1_000_000_000_000,
        cpu_usage_pct: 20.0,
        ram_used_mib: 2048,
        ram_total_mib: 8192,
        gpu_utilization_pct: None,
        vram_used_mib: None,
        vram_total_mib: None,
        models_loaded: 1,
    };
    insert_system_metric(&conn, &row1, 0).unwrap();

    let row2 = SystemMetricsRow {
        ts_unix_ms: 3_000_000_000_000,
        cpu_usage_pct: 40.0,
        ram_used_mib: 4096,
        ram_total_mib: 8192,
        gpu_utilization_pct: Some(60),
        vram_used_mib: Some(2048),
        vram_total_mib: Some(8192),
        models_loaded: 2,
    };
    insert_system_metric(&conn, &row2, 0).unwrap();

    // Get metrics since 2_000_000_000_000 (exclusive)
    let since = get_system_metrics_since(&conn, 2_000_000_000_000).unwrap();
    assert_eq!(since.len(), 1);
    assert_eq!(since[0].ts_unix_ms, 3_000_000_000_000);
}
