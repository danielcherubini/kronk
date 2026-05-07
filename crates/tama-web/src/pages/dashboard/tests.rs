use super::*;

/// `MetricSample` must deserialize a payload that has no `models` field at
/// all (older backend builds, cached responses) by defaulting to an empty
/// `Vec`. The `#[serde(default)]` attribute on the field is what makes this
/// work — without it, deserialization would fail with a `missing field`
/// error and break the dashboard during a partial rollout.
#[test]
fn metric_sample_deserializes_without_models_field() {
    let json = r#"{
        "ts_unix_ms": 1700000000000,
        "cpu_usage_pct": 12.5,
        "ram_used_mib": 2048,
        "ram_total_mib": 16384,
        "gpu_utilization_pct": null,
        "vram": null,
        "models_loaded": 0
    }"#;

    let sample: MetricSample = serde_json::from_str(json)
        .expect("MetricSample without `models` must deserialize via #[serde(default)]");

    assert_eq!(sample.ts_unix_ms, 1_700_000_000_000);
    assert_eq!(sample.cpu_usage_pct, 12.5);
    assert_eq!(sample.ram_used_mib, 2048);
    assert_eq!(sample.ram_total_mib, 16_384);
    assert!(sample.gpu_utilization_pct.is_none());
    assert!(sample.vram.is_none());
    assert_eq!(sample.models_loaded, 0);
    assert!(
        sample.models.is_empty(),
        "missing `models` field must default to an empty Vec"
    );
}

/// The `format_number` helper must produce comma-separated thousands.
#[test]
fn format_number_adds_commas() {
    assert_eq!(format_number(0), "0");
    assert_eq!(format_number(999), "999");
    assert_eq!(format_number(1000), "1,000");
    assert_eq!(format_number(12345), "12,345");
    assert_eq!(format_number(123456), "123,456");
    assert_eq!(format_number(1234567), "1,234,567");
    assert_eq!(format_number(16384), "16,384");
    assert_eq!(format_number(65183), "65,183");
}

/// `active_models` returns entries whose state is "ready", "loading", or
/// "unloading", preserving order and including all fields.
#[test]
fn active_models_returns_ready_loading_unloading_entries() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "c".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "ik_llama".into(),
            state: "loading".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "d".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "ik_llama".into(),
            state: "failed".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "e".into(),
            db_id: None,
            api_name: None,
            display_name: None,
            backend: "llama_cpp".into(),
            state: "unloading".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert_eq!(active.len(), 3);
    assert_eq!(active[0].id, "a");
    assert_eq!(active[0].state, "ready");
    assert_eq!(active[1].id, "c");
    assert_eq!(active[1].state, "loading");
    assert_eq!(active[2].id, "e");
    assert_eq!(active[2].state, "unloading");
}

/// `active_models` includes ready, loading, and unloading models.
#[test]
fn active_models_filters_to_active_states() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "c".into(),
            state: "loading".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "d".into(),
            state: "unloading".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert_eq!(active.len(), 3);
    assert_eq!(active[0].id, "a");
    assert_eq!(active[1].id, "c");
    assert_eq!(active[2].id, "d");
}

/// `active_models` returns an empty vec when all models are idle or failed.
#[test]
fn active_models_returns_empty_when_none_active() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "failed".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert!(active.is_empty());
}

/// `active_models` returns a clone of all models when all are active.
#[test]
fn active_models_returns_all_when_all_active() {
    let models = vec![
        ModelStatus {
            id: "x".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "y".into(),
            state: "loading".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    assert_eq!(active.len(), 2);
    assert_eq!(active[0].id, "x");
    assert_eq!(active[1].id, "y");
}

/// `active_models` returns an empty vec for an empty input slice.
#[test]
fn active_models_returns_empty_for_empty_input() {
    let models: Vec<ModelStatus> = vec![];
    let active = active_models(&models);
    assert!(active.is_empty());
}

/// `inactive_models` returns entries whose state is NOT "ready", "loading",
/// or "unloading" — i.e. idle, failed, and any unknown states.
#[test]
fn inactive_models_returns_idle_failed_and_unknown_entries() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "c".into(),
            state: "loading".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "d".into(),
            state: "failed".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "e".into(),
            state: "unloading".into(),
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 2);
    assert_eq!(inactive[0].id, "b");
    assert_eq!(inactive[0].state, "idle");
    assert_eq!(inactive[1].id, "d");
    assert_eq!(inactive[1].state, "failed");
}

/// `inactive_models` returns an empty vec when all models are active
/// (ready, loading, or unloading).
#[test]
fn inactive_models_returns_empty_when_all_active() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "loading".into(),
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert!(inactive.is_empty());
}

/// `inactive_models` returns all models when none are active.
#[test]
fn inactive_models_returns_all_when_none_active() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "failed".into(),
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 2);
    assert_eq!(inactive[0].id, "a");
    assert_eq!(inactive[1].id, "b");
}

/// `inactive_models` returns an empty vec for an empty input slice.
#[test]
fn inactive_models_returns_empty_for_empty_input() {
    let models: Vec<ModelStatus> = vec![];
    let inactive = inactive_models(&models);
    assert!(inactive.is_empty());
}

/// `inactive_models` preserves all model fields (display_name, quant,
/// context_length, db_id, backend) so the Inactive Models section can
/// render them without any data loss.
#[test]
fn inactive_models_preserves_all_fields() {
    let models = vec![
        ModelStatus {
            id: "llama3-8b".into(),
            db_id: Some(1),
            api_name: Some("meta-llama/Llama-3-8B".into()),
            display_name: Some("Llama 3 8B".into()),
            backend: "llama_cpp".into(),
            state: "ready".into(),
            quant: Some("Q4_K_M".into()),
            context_length: Some(8192),
            ..Default::default()
        },
        ModelStatus {
            id: "mistral-7b".into(),
            db_id: Some(2),
            api_name: Some("mistralai/Mistral-7B".into()),
            display_name: Some("Mistral 7B".into()),
            backend: "llama_cpp".into(),
            state: "idle".into(),
            quant: Some("Q4_0".into()),
            context_length: Some(32768),
            ..Default::default()
        },
        ModelStatus {
            id: "gemma-2b".into(),
            db_id: Some(3),
            api_name: Some("google/gemma-2b".into()),
            display_name: Some("Gemma 2B".into()),
            backend: "llama_cpp".into(),
            state: "failed".into(),
            quant: Some("Q5_K_M".into()),
            context_length: Some(4096),
            ..Default::default()
        },
    ];

    let inactive = inactive_models(&models);
    assert_eq!(inactive.len(), 2);

    // Verify idle model fields are preserved
    let idle_model = &inactive
        .iter()
        .find(|m| m.state == "idle")
        .expect("idle model missing");
    assert_eq!(idle_model.id, "mistral-7b");
    assert_eq!(idle_model.db_id, Some(2));
    assert_eq!(idle_model.display_name, Some("Mistral 7B".into()));
    assert_eq!(idle_model.quant, Some("Q4_0".into()));
    assert_eq!(idle_model.context_length, Some(32768));
    assert_eq!(idle_model.backend, "llama_cpp");

    // Verify failed model fields are preserved
    let failed_model = &inactive
        .iter()
        .find(|m| m.state == "failed")
        .expect("failed model missing");
    assert_eq!(failed_model.id, "gemma-2b");
    assert_eq!(failed_model.db_id, Some(3));
    assert_eq!(failed_model.display_name, Some("Gemma 2B".into()));
    assert_eq!(failed_model.quant, Some("Q5_K_M".into()));
    assert_eq!(failed_model.context_length, Some(4096));
    assert_eq!(failed_model.backend, "llama_cpp");
}

/// `active_models` and `inactive_models` are symmetric complements:
/// together they must contain exactly all input models, with no overlap.
#[test]
fn active_and_inactive_models_are_symmetric_complements() {
    let models = vec![
        ModelStatus {
            id: "a".into(),
            state: "ready".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "b".into(),
            state: "idle".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "c".into(),
            state: "loading".into(),
            ..Default::default()
        },
        ModelStatus {
            id: "d".into(),
            state: "failed".into(),
            ..Default::default()
        },
    ];

    let active = active_models(&models);
    let inactive = inactive_models(&models);

    assert_eq!(active.len() + inactive.len(), models.len());

    // No overlap: no model id appears in both lists.
    let active_ids: Vec<&str> = active.iter().map(|m| m.id.as_str()).collect();
    for inactive_model in &inactive {
        assert!(
            !active_ids.contains(&inactive_model.id.as_str()),
            "model '{}' should not be in both active and inactive",
            inactive_model.id
        );
    }
}

/// When the backend includes a populated `models` array, every `ModelStatus`
/// must round-trip with its `id`, `backend`, and `state` fields preserved.
#[test]
fn metric_sample_deserializes_models_field() {
    let json = r#"{
        "ts_unix_ms": 1700000000000,
        "cpu_usage_pct": 0.0,
        "ram_used_mib": 0,
        "ram_total_mib": 0,
        "gpu_utilization_pct": null,
        "vram": null,
        "models_loaded": 1,
        "models": [
            { "id": "alpha", "api_name": "org/alpha", "backend": "llama_cpp", "loaded": true, "state": "ready" },
            { "id": "beta",  "api_name": "org/beta",  "backend": "ik_llama",  "loaded": false, "state": "idle" }
        ]
    }"#;

    let sample: MetricSample =
        serde_json::from_str(json).expect("MetricSample with `models` must deserialize");

    assert_eq!(sample.models.len(), 2);

    assert_eq!(sample.models[0].id, "alpha");
    assert_eq!(sample.models[0].api_name, Some("org/alpha".to_string()));
    assert_eq!(sample.models[0].backend, "llama_cpp");
    assert_eq!(sample.models[0].state, "ready");

    assert_eq!(sample.models[1].id, "beta");
    assert_eq!(sample.models[1].api_name, Some("org/beta".to_string()));
    assert_eq!(sample.models[1].backend, "ik_llama");
    assert_eq!(sample.models[1].state, "idle");
}
