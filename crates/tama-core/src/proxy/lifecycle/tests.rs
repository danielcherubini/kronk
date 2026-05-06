use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::proxy::types::{ModelState, ProxyState};
use std::time::Instant;

/// Helper to create a Ready ModelState for testing.
/// Uses a high PID that won't exist and won't conflict with real processes.
fn make_ready_state(model_name: &str, backend: &str) -> ModelState {
    ModelState::Ready {
        model_name: model_name.to_string(),
        backend: backend.to_string(),
        backend_pid: 12345, // fake PID — won't be killed by tests
        backend_url: "http://127.0.0.1:8080".to_string(),
        load_time: std::time::SystemTime::now(),
        last_accessed: Instant::now(),
        consecutive_failures: Arc::new(AtomicU32::new(0)),
        failure_timestamp: None,
        restart_count: 0,
    }
}

/// Helper to create a Starting ModelState for testing.
fn make_starting_state(model_name: &str, backend: &str) -> ModelState {
    ModelState::Starting {
        model_name: model_name.to_string(),
        backend: backend.to_string(),
        backend_url: String::new(),
        backend_pid: 0,
        last_accessed: Instant::now(),
        start_time: Instant::now(),
        consecutive_failures: Arc::new(AtomicU32::new(0)),
        failure_timestamp: None,
    }
}

/// Helper to create a Failed ModelState for testing.
fn make_failed_state() -> ModelState {
    ModelState::Failed {
        model_name: "failed-model".to_string(),
        backend: "llama-cpp".to_string(),
        error: "test error".to_string(),
    }
}

/// Helper to create an Unloading ModelState for testing.
fn make_unloading_state(model_name: &str, backend: &str) -> ModelState {
    ModelState::Unloading {
        model_name: model_name.to_string(),
        backend: backend.to_string(),
        backend_pid: 54321,
        backend_url: "http://127.0.0.1:9000".to_string(),
        last_accessed: Instant::now(),
        consecutive_failures: Arc::new(AtomicU32::new(0)),
        failure_timestamp: None,
        restart_count: 0,
    }
}

/// Test that Starting state servers are skipped during idle check.
#[tokio::test]
async fn test_starting_state_skipped_in_idle_check() {
    let config = Config::default();
    let state = ProxyState::new(config, None);
    state.models.write().await.insert(
        "test-server".to_string(),
        make_starting_state("model.gguf", "llama-cpp"),
    );

    let result = state.check_idle_timeouts().await;
    assert!(
        result.is_empty(),
        "Starting servers should be skipped in idle check"
    );
}

/// Test that Failed servers without last_accessed are marked for cleanup.
#[tokio::test]
async fn test_failed_server_marked_for_cleanup() {
    let config = Config::default();
    let state = ProxyState::new(config, None);
    state
        .models
        .write()
        .await
        .insert("failed-server".to_string(), make_failed_state());

    let result = state.check_idle_timeouts().await;
    assert!(
        result.contains(&"failed-server".to_string()),
        "Failed servers should be marked for cleanup"
    );
}

/// Test ModelState::is_ready() returns correct values for each variant.
#[test]
fn test_model_state_is_ready() {
    let ready = make_ready_state("m", "llama-cpp");
    assert!(ready.is_ready());

    let starting = make_starting_state("m", "llama-cpp");
    assert!(!starting.is_ready());

    let failed = make_failed_state();
    assert!(!failed.is_ready());
}

/// Test ModelState::last_accessed() returns correct values.
#[test]
fn test_model_state_last_accessed() {
    let ready = make_ready_state("m", "llama-cpp");
    assert!(ready.last_accessed().is_some());

    let starting = make_starting_state("m", "llama-cpp");
    assert!(starting.last_accessed().is_some());

    // Failed state has no last_accessed
    let failed = make_failed_state();
    assert!(failed.last_accessed().is_none());
}

/// Test ModelState::backend() returns the correct backend name.
#[test]
fn test_model_state_backend() {
    let ready = make_ready_state("m", "llama-cpp-cuda");
    assert_eq!(ready.backend(), "llama-cpp-cuda");

    let starting = make_starting_state("m", "vllm");
    assert_eq!(starting.backend(), "vllm");
}

/// Test ModelState::backend_pid() returns the correct PID.
#[test]
fn test_model_state_backend_pid() {
    let ready = make_ready_state("m", "llama-cpp");
    assert_eq!(ready.backend_pid(), Some(12345));

    let starting = make_starting_state("m", "llama-cpp");
    assert_eq!(starting.backend_pid(), Some(0));

    let failed = make_failed_state();
    assert!(failed.backend_pid().is_none());
}

/// Test that consecutive_failures counter is accessible.
#[test]
fn test_model_state_consecutive_failures() {
    let ready = make_ready_state("m", "llama-cpp");
    let failures = ready.consecutive_failures();
    assert!(failures.is_some());
    assert_eq!(failures.unwrap().load(Ordering::Relaxed), 0);
}

/// Test that ModelState::is_ready() distinguishes all variants correctly.
#[test]
fn test_model_state_variants() {
    let ready = make_ready_state("m", "llama-cpp");
    assert!(matches!(ready, ModelState::Ready { .. }));

    let starting = make_starting_state("m", "llama-cpp");
    assert!(matches!(starting, ModelState::Starting { .. }));

    let failed = make_failed_state();
    assert!(matches!(failed, ModelState::Failed { .. }));
}

/// Test that can_reload() returns true when no failure timestamp is set.
#[test]
fn test_can_reload_no_failure_timestamp() {
    let ready = make_ready_state("m", "llama-cpp");
    assert!(ready.can_reload(60));
}

/// Test that can_reload() returns true when cooldown has elapsed.
#[test]
fn test_can_reload_cooldown_elapsed() {
    let mut ready = make_ready_state("m", "llama-cpp");
    if let ModelState::Ready {
        failure_timestamp, ..
    } = &mut ready
    {
        *failure_timestamp = Some(std::time::SystemTime::now() - Duration::from_secs(120));
    }
    assert!(ready.can_reload(60));
}

/// Test that can_reload() returns false when cooldown is active.
#[test]
fn test_can_reload_cooldown_active() {
    let mut ready = make_ready_state("m", "llama-cpp");
    if let ModelState::Ready {
        failure_timestamp, ..
    } = &mut ready
    {
        *failure_timestamp = Some(std::time::SystemTime::now());
    }
    assert!(!ready.can_reload(60));
}

/// Test that Unloading state model_name() returns the correct name.
#[test]
fn test_unloading_model_name() {
    let unloading = make_unloading_state("unload-model", "llama-cpp");
    assert_eq!(unloading.model_name(), "unload-model");
}

/// Test that Unloading state backend() returns the correct backend.
#[test]
fn test_unloading_backend() {
    let unloading = make_unloading_state("m", "vllm");
    assert_eq!(unloading.backend(), "vllm");
}

/// Test that Unloading state is_ready() returns false.
#[test]
fn test_unloading_is_not_ready() {
    let unloading = make_unloading_state("m", "llama-cpp");
    assert!(!unloading.is_ready());
}

/// Test that Unloading state backend_url() returns None.
#[test]
fn test_unloading_backend_url_none() {
    let unloading = make_unloading_state("m", "llama-cpp");
    assert!(unloading.backend_url().is_none());
}

/// Test that Unloading state backend_pid() returns the PID.
#[test]
fn test_unloading_backend_pid() {
    let unloading = make_unloading_state("m", "llama-cpp");
    assert_eq!(unloading.backend_pid(), Some(54321));
}

/// Test that Unloading state consecutive_failures() returns the counter.
#[test]
fn test_unloading_consecutive_failures() {
    let unloading = make_unloading_state("m", "llama-cpp");
    let failures = unloading.consecutive_failures();
    assert!(failures.is_some());
    assert_eq!(failures.unwrap().load(Ordering::Relaxed), 0);
}

/// Test that Unloading state load_time() returns None.
#[test]
fn test_unloading_load_time_none() {
    let unloading = make_unloading_state("m", "llama-cpp");
    assert!(unloading.load_time().is_none());
}

/// Test that Unloading state last_accessed() returns Some.
#[test]
fn test_unloading_last_accessed() {
    let unloading = make_unloading_state("m", "llama-cpp");
    assert!(unloading.last_accessed().is_some());
}

/// Test that Unloading state can_reload() returns false.
#[test]
fn test_unloading_can_reload_false() {
    let unloading = make_unloading_state("m", "llama-cpp");
    assert!(!unloading.can_reload(60));
}

/// Test that ModelState::Default produces a Failed state with empty strings.
#[test]
fn test_model_state_default_is_failed() {
    let default_state = ModelState::default();
    assert!(!default_state.is_ready());
    assert_eq!(default_state.model_name(), "");
    assert_eq!(default_state.backend(), "");
}

/// Test that Unloading state matches correctly.
#[test]
fn test_unloading_variant_match() {
    let unloading = make_unloading_state("m", "llama-cpp");
    assert!(matches!(unloading, ModelState::Unloading { .. }));
}

/// Test that evict_lru_if_needed returns Ok(None) when max_loaded_models is 0 (unlimited).
#[tokio::test]
async fn test_evict_lru_if_needed_zero_is_unlimited() {
    let mut config = Config::default();
    config.proxy.max_loaded_models = 0;
    let state = ProxyState::new(config, None);

    // Add a Ready model to ensure we're not returning None due to empty map
    state.models.write().await.insert(
        "server1".to_string(),
        make_ready_state("model.gguf", "llama-cpp"),
    );

    let result = state.evict_lru_if_needed().await;
    assert!(
        result.is_ok(),
        "evict_lru_if_needed should succeed with unlimited config"
    );
    assert_eq!(
        result.unwrap(),
        None,
        "Should return None when max_loaded_models is 0"
    );
}

/// Test that evict_lru_if_needed returns Ok(None) when model count is below the limit.
#[tokio::test]
async fn test_evict_lru_if_needed_under_limit_no_eviction() {
    let mut config = Config::default();
    config.proxy.max_loaded_models = 2;
    let state = ProxyState::new(config, None);

    // Add 1 Ready model (below limit of 2)
    state.models.write().await.insert(
        "server1".to_string(),
        make_ready_state("model.gguf", "llama-cpp"),
    );

    let result = state.evict_lru_if_needed().await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None, "Should return None when under limit");

    // Verify model count is unchanged
    assert_eq!(
        state.models.read().await.len(),
        1,
        "Model count should be unchanged"
    );
}

/// Test that evict_lru_if_needed evicts the LRU Ready model when at capacity.
#[tokio::test]
async fn test_evict_lru_if_needed_at_limit_evicts_lru() {
    let mut config = Config::default();
    config.proxy.max_loaded_models = 1;
    let state = ProxyState::new(config, None);

    // Add a Ready model with last_accessed set in the past
    let mut ready_state = make_ready_state("model.gguf", "llama-cpp");
    if let ModelState::Ready { last_accessed, .. } = &mut ready_state {
        *last_accessed = Instant::now() - Duration::from_secs(300);
    }
    state
        .models
        .write()
        .await
        .insert("server1".to_string(), ready_state);

    let result = state.evict_lru_if_needed().await;
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        Some("server1".to_string()),
        "Should evict the only Ready model when at capacity"
    );

    // Verify model was removed from the map
    assert!(
        !state.models.read().await.contains_key("server1"),
        "Evicted model should be removed from the map"
    );
}

/// Test that evict_lru_if_needed skips Starting models.
#[tokio::test]
async fn test_evict_lru_if_needed_skips_starting_models() {
    let mut config = Config::default();
    config.proxy.max_loaded_models = 1;
    let state = ProxyState::new(config, None);

    // Add a Starting model (not Ready)
    state.models.write().await.insert(
        "server1".to_string(),
        make_starting_state("model.gguf", "llama-cpp"),
    );

    let result = state.evict_lru_if_needed().await;
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        None,
        "Should return None when no Ready models are available"
    );

    // Verify Starting model remains in the map
    assert!(
        state.models.read().await.contains_key("server1"),
        "Starting model should remain in the map"
    );
}

/// Test that evict_lru_if_needed skips Failed models.
#[tokio::test]
async fn test_evict_lru_if_needed_skips_failed_models() {
    let mut config = Config::default();
    config.proxy.max_loaded_models = 1;
    let state = ProxyState::new(config, None);

    // Add a Failed model
    state
        .models
        .write()
        .await
        .insert("server1".to_string(), make_failed_state());

    let result = state.evict_lru_if_needed().await;
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        None,
        "Should return None when no Ready models are available"
    );
}

/// Test that concurrent evict calls don't double-evict the same model.
/// With max_loaded_models=1 and 3 models (2 Ready + 1 Starting), each call
/// finds a different Ready model since the Starting model is skipped.
#[tokio::test]
async fn test_evict_lru_if_needed_concurrent_no_double_eviction() {
    let mut config = Config::default();
    config.proxy.max_loaded_models = 1;
    let state = ProxyState::new(config, None);

    // Add 2 Ready models with different last_accessed times (LRU + newer)
    let mut ready1 = make_ready_state("model1.gguf", "llama-cpp");
    if let ModelState::Ready { last_accessed, .. } = &mut ready1 {
        *last_accessed = Instant::now() - Duration::from_secs(600); // older
    }
    state
        .models
        .write()
        .await
        .insert("server1".to_string(), ready1);

    let mut ready2 = make_ready_state("model2.gguf", "llama-cpp");
    if let ModelState::Ready { last_accessed, .. } = &mut ready2 {
        *last_accessed = Instant::now() - Duration::from_secs(100); // newer
    }
    state
        .models
        .write()
        .await
        .insert("server2".to_string(), ready2);

    // Add 1 Starting model — it should be skipped by eviction, ensuring
    // both concurrent calls have a Ready model to evict.
    state.models.write().await.insert(
        "server3".to_string(),
        make_starting_state("model3.gguf", "llama-cpp"),
    );

    // Run two evict calls concurrently
    let state_a = state.clone();
    let state_b = state.clone();
    let handle_a = tokio::spawn(async move { state_a.evict_lru_if_needed().await });
    let handle_b = tokio::spawn(async move { state_b.evict_lru_if_needed().await });

    let result_a = handle_a.await.unwrap();
    let result_b = handle_b.await.unwrap();

    // Both calls should succeed (each evicts a different Ready model)
    assert!(result_a.is_ok());
    assert!(result_b.is_ok());

    // Each call returns a different server name — no double-eviction
    let name_a = result_a.unwrap().unwrap();
    let name_b = result_b.unwrap().unwrap();
    assert_ne!(
        name_a, name_b,
        "Concurrent calls must evict different models (no double-eviction)"
    );

    // Both evicted models should be removed from the map
    assert!(
        !state.models.read().await.contains_key(&name_a),
        "Evicted model '{}' should be removed",
        name_a
    );
    assert!(
        !state.models.read().await.contains_key(&name_b),
        "Evicted model '{}' should be removed",
        name_b
    );
}

/// Test that TTS backends are excluded from LRU eviction count.
#[tokio::test]
async fn test_evict_lru_excludes_tts_backends() {
    use crate::config::ModelConfig;

    let mut config = Config::default();
    config.proxy.max_loaded_models = 1;
    let state = ProxyState::new(config, None);

    // Register the TTS server in model_configs with a tts_ backend
    // so it's excluded from the LLM count.
    state.model_configs.write().await.insert(
        "tts-server".to_string(),
        ModelConfig {
            backend: "tts_kokoro".to_string(),
            ..Default::default()
        },
    );

    // Add a TTS backend (tts_kokoro) — should NOT count toward limit
    let tts_state = make_ready_state("model.gguf", "tts_kokoro");
    state
        .models
        .write()
        .await
        .insert("tts-server".to_string(), tts_state);

    // Verify no eviction happens (TTS doesn't count)
    let result = state.evict_lru_if_needed().await.unwrap();
    assert_eq!(result, None, "TTS backends should not trigger eviction");

    // Verify the TTS model is still in the map
    assert!(
        state.models.read().await.contains_key("tts-server"),
        "TTS backend should remain loaded"
    );
}
