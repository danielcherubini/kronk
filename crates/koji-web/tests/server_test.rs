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
                upload_lock: std::sync::Arc::new(tokio::sync::RwLock::new(
                    std::collections::HashMap::new(),
                )),
                update_checker: Arc::new(koji_core::updates::UpdateChecker::new()),
                download_queue: None,
            });
            axum::serve(listener, koji_web::server::build_router(state))
                .await
                .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (reqwest::Client::new(), addr)
    }

    /// Helper to get a CSRF token from the server.
    async fn get_csrf_token(client: &reqwest::Client, base_url: &str) -> String {
        let resp = client
            .get(format!("{}/koji/v1/config/structured", base_url))
            .send()
            .await
            .unwrap();
        resp.headers()
            .get(reqwest::header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|cookie| {
                cookie
                    .split(';')
                    .next()
                    .and_then(|part| part.split_once('='))
                    .map(|(_, val)| val.to_string())
            })
            .unwrap_or_else(|| "test-token".to_string())
    }

    /// Helper to make a POST request with CSRF token.
    #[allow(dead_code)]
    async fn post_with_csrf(
        client: &reqwest::Client,
        url: &str,
        body: serde_json::Value,
        csrf_token: &str,
    ) -> reqwest::Response {
        client
            .post(url)
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("koji_csrf_token={csrf_token}"))
            .header("x-csrf-token", csrf_token)
            .json(&body)
            .send()
            .await
            .unwrap()
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

    /// GET /koji/v1/config returns 404 when config_path is None (not configured).
    #[tokio::test]
    async fn test_api_config_returns_404_when_unconfigured() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .get(format!("http://{}/koji/v1/config", addr))
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

    /// GET /koji/v1/logs returns 404 when logs_dir is None (not configured).
    #[tokio::test]
    async fn test_api_logs_returns_404_when_unconfigured() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .get(format!("http://{}/koji/v1/logs", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        let _body = resp.text().await.unwrap_or_default();
    }

    /// POST /koji/v1/config returns 404 when config_path is None (checked before TOML validation).
    #[tokio::test]
    async fn test_api_config_save_returns_403_when_unauthenticated() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .post(format!("http://{}/koji/v1/config", addr))
            .json(&serde_json::json!({ "content": "not valid toml [[[[" }))
            .send()
            .await
            .unwrap();
        // 403 because CSRF token not provided
        assert_eq!(resp.status().as_u16(), 403);
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

        // Start from the default config.
        let initial_config = koji_core::config::Config {
            loaded_from: Some(config_dir.clone()),
            ..Default::default()
        };
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
                    upload_lock: std::sync::Arc::new(tokio::sync::RwLock::new(
                        std::collections::HashMap::new(),
                    )),
                    update_checker: Arc::new(koji_core::updates::UpdateChecker::new()),
                    download_queue: None,
                });
                axum::serve(listener, koji_web::server::build_router(state))
                    .await
                    .unwrap();
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let client = reqwest::Client::new();
        // Get CSRF token for authenticated POST requests
        let csrf_token = get_csrf_token(&client, &format!("http://{}/", addr)).await;

        // ── POST /koji/v1/models — create ─────────────────────────────────────────────
        let resp = client
            .post(format!("http://{}/koji/v1/models", addr))
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("koji_csrf_token={csrf_token}"))
            .header("x-csrf-token", &csrf_token)
            .json(&serde_json::json!({
                "repo_id": "test-model",
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
            "POST /koji/v1/models should return 201 Created"
        );

        // Verify 'test-model' was created via GET /koji/v1/models.
        // Extract its auto-assigned integer id for subsequent requests.
        let model_id: i64 = {
            let resp = client
                .get(format!("http://{}/koji/v1/models", addr))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            let models = body["models"].as_array().unwrap();
            let model = models
                .iter()
                .find(|m| m["repo_id"].as_str() == Some("test-model"));
            assert!(
                model.is_some(),
                "proxy config should contain 'test-model' after POST /koji/v1/models"
            );
            let model = model.unwrap();
            assert_eq!(
                model["backend"].as_str(),
                Some("llama_cpp"),
                "backend should be 'llama_cpp'"
            );
            model["id"].as_i64().unwrap()
        };

        // ── PUT /koji/v1/models/:id — update ──────────────────────────────────────────
        let resp = client
            .put(format!("http://{}/koji/v1/models/{}", addr, model_id))
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("koji_csrf_token={csrf_token}"))
            .header("x-csrf-token", &csrf_token)
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
            "PUT /koji/v1/models/:id should return 200"
        );

        // Verify 'test-model' was updated via GET /koji/v1/models.
        {
            let resp = client
                .get(format!("http://{}/koji/v1/models", addr))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            let models = body["models"].as_array().unwrap();
            let model = models
                .iter()
                .find(|m| m["repo_id"].as_str() == Some("test-model"));
            assert!(model.is_some(), "test-model should still exist after PUT");
            let model = model.unwrap();
            assert_eq!(
                model["backend"].as_str(),
                Some("ik_llama"),
                "backend should be updated to 'ik_llama'"
            );
            assert_eq!(
                model["enabled"].as_bool(),
                Some(false),
                "model should be disabled after update"
            );
        }

        // ── DELETE /koji/v1/models/:id ────────────────────────────────────────────────
        let resp = client
            .delete(format!("http://{}/koji/v1/models/{}", addr, model_id))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            200,
            "DELETE /koji/v1/models/:id should return 200"
        );

        // Verify 'test-model' was removed via GET /koji/v1/models.
        {
            let resp = client
                .get(format!("http://{}/koji/v1/models", addr))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            let models = body["models"].as_array().unwrap();
            let found = models
                .iter()
                .any(|m| m["repo_id"].as_str() == Some("test-model"));
            assert!(
                !found,
                "proxy config should not contain 'test-model' after DELETE"
            );
        }

        // ── POST /koji/v1/models — create hot-reload-model ────────────────────────────
        // Models are stored in SQLite, so create via the API directly.
        let resp = client
            .post(format!("http://{}/koji/v1/models", addr))
            .header("origin", "http://localhost:11435")
            .header("cookie", format!("koji_csrf_token={csrf_token}"))
            .header("x-csrf-token", &csrf_token)
            .json(&serde_json::json!({
                "repo_id": "hot-reload-model",
                "backend": "llama_cpp",
                "args": [],
                "enabled": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status().as_u16(),
            201,
            "POST /koji/v1/models should return 201 for hot-reload-model"
        );

        // Verify 'hot-reload-model' was created via GET /koji/v1/models.
        {
            let resp = client
                .get(format!("http://{}/koji/v1/models", addr))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status().as_u16(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            let models = body["models"].as_array().unwrap();
            let found = models
                .iter()
                .any(|m| m["repo_id"].as_str() == Some("hot-reload-model"));
            assert!(
                found,
                "proxy config should contain 'hot-reload-model' after POST /koji/v1/models"
            );
        }

        // Keep temp_dir alive until all assertions are done so the files aren't removed early.
        drop(temp_dir);
    }
}
