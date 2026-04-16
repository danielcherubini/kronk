//! Tests for database query functions.

use crate::db::{open_in_memory, OpenResult};

use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_and_get_update_check() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let item_type = "backend";
        let item_id = "llama-cpp";
        let now = 1713168000; // 2024-04-15

        // Insert
        upsert_update_check(
            &conn,
            super::update_check_queries::UpdateCheckParams {
                item_type,
                item_id,
                current_version: Some("v1.0.0"),
                latest_version: Some("v1.1.0"),
                update_available: true,
                status: "update_available",
                error_message: None,
                details_json: None,
                checked_at: now,
            },
        )
        .unwrap();

        let record = get_update_check(&conn, item_type, item_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.item_type, item_type);
        assert_eq!(record.item_id, item_id);
        assert_eq!(record.current_version.unwrap(), "v1.0.0");
        assert_eq!(record.latest_version.unwrap(), "v1.1.0");
        assert!(record.update_available);
        assert_eq!(record.status, "update_available");
        assert_eq!(record.checked_at, now);

        // Upsert (Update)
        upsert_update_check(
            &conn,
            super::update_check_queries::UpdateCheckParams {
                item_type,
                item_id,
                current_version: Some("v1.1.0"),
                latest_version: Some("v1.1.0"),
                update_available: false,
                status: "up_to_date",
                error_message: None,
                details_json: None,
                checked_at: now + 100,
            },
        )
        .unwrap();

        let updated = get_update_check(&conn, item_type, item_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.current_version.unwrap(), "v1.1.0");
        assert!(!updated.update_available);
        assert_eq!(updated.status, "up_to_date");
        assert_eq!(updated.checked_at, now + 100);
    }

    #[test]
    fn test_get_all_update_checks() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let now = 1713168000;

        upsert_update_check(
            &conn,
            super::update_check_queries::UpdateCheckParams {
                item_type: "backend",
                item_id: "b1",
                current_version: None,
                latest_version: None,
                update_available: false,
                status: "unknown",
                error_message: None,
                details_json: None,
                checked_at: now,
            },
        )
        .unwrap();

        upsert_update_check(
            &conn,
            super::update_check_queries::UpdateCheckParams {
                item_type: "model",
                item_id: "m1",
                current_version: None,
                latest_version: None,
                update_available: false,
                status: "unknown",
                error_message: None,
                details_json: None,
                checked_at: now,
            },
        )
        .unwrap();

        let all = get_all_update_checks(&conn).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_delete_update_check() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let item_type = "backend";
        let item_id = "b1";

        upsert_update_check(
            &conn,
            super::update_check_queries::UpdateCheckParams {
                item_type,
                item_id,
                current_version: None,
                latest_version: None,
                update_available: false,
                status: "unknown",
                error_message: None,
                details_json: None,
                checked_at: 12345,
            },
        )
        .unwrap();

        delete_update_check(&conn, item_type, item_id).unwrap();
        let record = get_update_check(&conn, item_type, item_id).unwrap();
        assert!(record.is_none());
    }

    #[test]
    fn test_get_oldest_check_time() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        assert_eq!(get_oldest_check_time(&conn).unwrap(), None);

        upsert_update_check(
            &conn,
            super::update_check_queries::UpdateCheckParams {
                item_type: "backend",
                item_id: "b1",
                current_version: None,
                latest_version: None,
                update_available: false,
                status: "unknown",
                error_message: None,
                details_json: None,
                checked_at: 2000,
            },
        )
        .unwrap();

        upsert_update_check(
            &conn,
            super::update_check_queries::UpdateCheckParams {
                item_type: "backend",
                item_id: "b2",
                current_version: None,
                latest_version: None,
                update_available: false,
                status: "unknown",
                error_message: None,
                details_json: None,
                checked_at: 1000,
            },
        )
        .unwrap();

        assert_eq!(get_oldest_check_time(&conn).unwrap(), Some(1000));
    }

    #[test]
    fn test_upsert_and_get_model_config() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let record = ModelConfigRecord {
            repo_id: "test-repo".to_string(),
            display_name: Some("Test Model".to_string()),
            backend: "llama_cpp".to_string(),
            enabled: true,
            selected_quant: Some("Q4_K_M".to_string()),
            selected_mmproj: Some("mmproj-f16.gguf".to_string()),
            context_length: Some(4096),
            gpu_layers: Some(32),
            port: Some(8080),
            args: Some(r#"["--flash-attn"]"#.to_string()),
            sampling: Some(r#"{"temp": 0.7}"#.to_string()),
            modalities: Some(r#"{ "input": ["text"], "output": ["text"] }"#.to_string()),
            profile: Some("default".to_string()),
            api_name: Some("test-api".to_string()),
            health_check: Some(r#"{"path": "/health"}"#.to_string()),
            created_at: "2024-04-15T12:00:00Z".to_string(),
            updated_at: "2024-04-15T12:00:00Z".to_string(),
        };

        upsert_model_config(&conn, &record).unwrap();

        let retrieved = get_model_config(&conn, "test-repo").unwrap().unwrap();
        assert_eq!(retrieved.repo_id, record.repo_id);
        assert_eq!(retrieved.display_name, record.display_name);
        assert_eq!(retrieved.backend, record.backend);
        assert_eq!(retrieved.enabled, record.enabled);
        assert_eq!(retrieved.selected_quant, record.selected_quant);
        assert_eq!(retrieved.selected_mmproj, record.selected_mmproj);
        assert_eq!(retrieved.context_length, record.context_length);
        assert_eq!(retrieved.gpu_layers, record.gpu_layers);
        assert_eq!(retrieved.port, record.port);
        assert_eq!(retrieved.args, record.args);
        assert_eq!(retrieved.sampling, record.sampling);
        assert_eq!(retrieved.modalities, record.modalities);
        assert_eq!(retrieved.profile, record.profile);
        assert_eq!(retrieved.api_name, record.api_name);
        assert_eq!(retrieved.health_check, record.health_check);
        assert_eq!(retrieved.created_at, record.created_at);
        // updated_at will be different because upsert_model_config updates it via strftime
    }

    #[test]
    fn test_get_all_model_configs() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let rec1 = ModelConfigRecord {
            repo_id: "repo1".to_string(),
            display_name: None,
            backend: "llama_cpp".to_string(),
            enabled: true,
            selected_quant: None,
            selected_mmproj: None,
            context_length: None,
            gpu_layers: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: None,
            health_check: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let rec2 = ModelConfigRecord {
            repo_id: "repo2".to_string(),
            display_name: None,
            backend: "llama_cpp".to_string(),
            enabled: true,
            selected_quant: None,
            selected_mmproj: None,
            context_length: None,
            gpu_layers: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: None,
            health_check: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };

        upsert_model_config(&conn, &rec1).unwrap();
        upsert_model_config(&conn, &rec2).unwrap();

        let all = get_all_model_configs(&conn).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_delete_model_config() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let record = ModelConfigRecord {
            repo_id: "test-repo".to_string(),
            display_name: None,
            backend: "llama_cpp".to_string(),
            enabled: true,
            selected_quant: None,
            selected_mmproj: None,
            context_length: None,
            gpu_layers: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: None,
            health_check: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        };

        upsert_model_config(&conn, &record).unwrap();
        assert!(get_model_config(&conn, "test-repo").unwrap().is_some());

        delete_model_config(&conn, "test-repo").unwrap();
        assert!(get_model_config(&conn, "test-repo").unwrap().is_none());
    }
}
