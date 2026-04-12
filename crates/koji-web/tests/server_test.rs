#[cfg(feature = "ssr")]
mod tests {
    use std::sync::Arc;

    async fn start_test_server() -> (reqwest::Client, std::net::SocketAddr) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let state = Arc::new(koji_web::server::AppState {
                jobs: None,
                capabilities: None,
                proxy_base_url: "http://127.0.0.1:11434".to_string(),
                client: reqwest::Client::new(),
                logs_dir: None,
                config_path: None,
                proxy_config: None,
                binary_version: "0.0.0-test".to_string(),
                update_tx: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            });
            axum::serve(listener, koji_web::server::build_router(state))
                .await
                .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (reqwest::Client::new(), addr)
    }

    /// GET / returns 200 (index.html embedded) or 404 (dist/ empty in dev) — both are valid.
    #[tokio::test]
    async fn test_root_returns_html_or_not_found() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .get(format!("http://{}/", addr))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        assert!(
            status == 200 || status == 404,
            "Expected 200 or 404 for /, got {status}"
        );
    }

    /// GET /api/config returns 404 when config_path is None (not configured).
    #[tokio::test]
    async fn test_api_config_returns_404_when_unconfigured() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .get(format!("http://{}/api/config", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body.get("error").is_some(),
            "Expected error field in response"
        );
    }

    /// GET /api/logs returns 404 when logs_dir is None (not configured).
    #[tokio::test]
    async fn test_api_logs_returns_404_when_unconfigured() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .get(format!("http://{}/api/logs", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body.get("error").is_some(),
            "Expected error field in response"
        );
    }

    /// POST /api/config returns 404 when config_path is None (checked before TOML validation).
    #[tokio::test]
    async fn test_api_config_save_returns_404_when_unconfigured() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .post(format!("http://{}/api/config", addr))
            .json(&serde_json::json!({ "content": "not valid toml [[[[" }))
            .send()
            .await
            .unwrap();
        // 404 because config_path is None (checked before TOML validation)
        assert_eq!(resp.status().as_u16(), 404);
    }

    /// End-to-end test: CRUD operations via the web API update the proxy's in-memory config.
    ///
    /// This verifies the hot-reload path: when a model is created, updated, or deleted
    /// through the web API, the proxy's live `Arc<RwLock<Config>>` is updated without
    /// requiring a restart.
    #[tokio::test]
    async fn test_hot_reload_crud_updates_proxy_config() {
        // ── Setup ─────────────────────────────────────────────────────────────────
        // Create a temporary config directory with a valid config.toml on disk.
        let temp_dir = tempfile::tempdir().unwrap();
        let config_dir = temp_dir.path().to_path_buf();
        let config_path = config_dir.join("config.toml");

        // Start from the default config but with an empty models table.
        let mut initial_config = koji_core::config::Config::default();
        initial_config.loaded_from = Some(config_dir.clone());
        initial_config.models.clear();
        let toml_str = toml::to_string_pretty(&initial_config).unwrap();
        std::fs::write(&config_path, &toml_str).unwrap();

        // The shared proxy config — this is what the proxy would hold in production.
        let proxy_config = Arc::new(tokio::sync::RwLock::new(initial_config));

        // ── Start server ──────────────────────────────────────────────────────────
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        {
            let proxy_config_server = proxy_config.clone();
            let config_path_server = config_path.clone();
            tokio::spawn(async move {
                let state = Arc::new(koji_web::server::AppState {
                    jobs: None,
                    capabilities: None,
                    proxy_base_url: "http://127.0.0.1:11434".to_string(),
                    client: reqwest::Client::new(),
                    logs_dir: None,
                    config_path: Some(config_path_server),
                    proxy_config: Some(proxy_config_server),
                    binary_version: "0.0.0-test".to_string(),
                    update_tx: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
                });
                axum::serve(listener, koji_web::server::build_router(state))
                    .await
                    .unwrap();
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let client = reqwest::Client::new();

        // ── POST /api/models — create ─────────────────────────────────────────────
        let resp = client
            .post(format!("http://{}/api/models", addr))
            .json(&serde_json::json!({
                "id": "test-model",
                "backend": "llama_cpp",
                "args": ["--host", "0.0.0.0"],
                "enabled": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            201,
            "POST /api/models should return 201 Created"
        );

        // Proxy config must now contain the new model.
        {
            let cfg = proxy_config.read().await;
            assert!(
                cfg.models.contains_key("test-model"),
                "proxy config should contain 'test-model' after POST /api/models"
            );
            assert_eq!(
                cfg.models["test-model"].backend, "llama_cpp",
                "backend should be 'llama_cpp'"
            );
        }

        // ── PUT /api/models/:id — update ──────────────────────────────────────────
        let resp = client
            .put(format!("http://{}/api/models/test-model", addr))
            .json(&serde_json::json!({
                "backend": "ik_llama",
                "args": [],
                "enabled": false
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            200,
            "PUT /api/models/:id should return 200"
        );

        // Proxy config must reflect the update.
        {
            let cfg = proxy_config.read().await;
            assert!(
                cfg.models.contains_key("test-model"),
                "test-model should still exist after PUT"
            );
            assert_eq!(
                cfg.models["test-model"].backend, "ik_llama",
                "backend should be updated to 'ik_llama'"
            );
            assert!(
                !cfg.models["test-model"].enabled,
                "model should be disabled after update"
            );
        }

        // ── DELETE /api/models/:id ────────────────────────────────────────────────
        let resp = client
            .delete(format!("http://{}/api/models/test-model", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            200,
            "DELETE /api/models/:id should return 200"
        );

        // Proxy config must no longer contain the model.
        {
            let cfg = proxy_config.read().await;
            assert!(
                !cfg.models.contains_key("test-model"),
                "proxy config should not contain 'test-model' after DELETE"
            );
        }

        // ── POST /api/config — replace config via raw TOML ────────────────────────
        // Build a valid config with a fresh model and serialise it to TOML.
        let mut new_config = koji_core::config::Config::default();
        new_config.models.clear();
        new_config.models.insert(
            "hot-reload-model".to_string(),
            koji_core::config::ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: None,
                quant: None,

                mmproj: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                api_name: Some("Hot Reload Test".to_string()),
                gpu_layers: None,
                quants: std::collections::BTreeMap::new(),
                modalities: None,
            },
        );
        let new_toml = toml::to_string_pretty(&new_config).unwrap();

        let resp = client
            .post(format!("http://{}/api/config", addr))
            .json(&serde_json::json!({ "content": new_toml }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            200,
            "POST /api/config should return 200"
        );

        // Proxy config must now reflect the brand-new config.
        {
            let cfg = proxy_config.read().await;
            assert!(
                cfg.models.contains_key("hot-reload-model"),
                "proxy config should contain 'hot-reload-model' after POST /api/config"
            );
            assert_eq!(
                cfg.models["hot-reload-model"].api_name,
                Some("Hot Reload Test".to_string()),
                "api_name should survive the hot-reload round-trip"
            );
        }

        // Keep temp_dir alive until all assertions are done so the files aren't removed early.
        drop(temp_dir);
    }
}
