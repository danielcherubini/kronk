//! Dynamic OpenAPI 3.1.0 spec generation from registered routes.
//! Served at `GET /koji/v1/docs` as JSON.

#[cfg(feature = "ssr")]
use axum::{http::StatusCode, response::IntoResponse, Json};

/// Returns the full OpenAPI 3.1.0 specification as a JSON value.
pub fn spec() -> serde_json::Value {
    let paths: std::collections::HashMap<String, serde_json::Value> = [
        // ── System ──────────────────────────────────────────────────────────────
        (
            "/koji/v1/system/capabilities",
            op(
                "get",
                "getCapabilities",
                "Get system capabilities",
                "Returns supported GPU architectures, CUDA/ROCm versions, and platform info.",
                &["system"],
                None,
                None,
            ),
        ),
        // ── Backends ────────────────────────────────────────────────────────────
        (
            "/koji/v1/backends",
            op(
                "get",
                "listBackends",
                "List all backends",
                "Returns configured and installed backends with their versions and states.",
                &["backends"],
                None,
                None,
            ),
        ),
        (
            "/koji/v1/backends/install",
            post_op(
                "installBackend",
                "Install a backend",
                "Installs a new backend version. Body limit: 16MB.",
                &["backends"],
                Some(("InstallRequest", "multipart/form-data")),
                Some("JobResponse"),
            ),
        ),
        (
            "/koji/v1/backends/{name}/update",
            post_op_p(
                "updateBackend",
                "Update a backend",
                "Updates an existing backend to its latest version. Body limit: 16MB.",
                &["backends"],
                &[("name", "path")],
                Some(("UpdateRequest", "application/json")),
                Some("JobResponse"),
            ),
        ),
        (
            "/koji/v1/backends/{name}",
            delete_op_p(
                "removeBackend",
                "Remove a backend",
                "Removes an installed backend.",
                &["backends"],
                &[("name", "path")],
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/backends/{name}/default-args",
            post_op_p(
                "updateBackendDefaultArgs",
                "Update default args",
                "Sets the default CLI arguments for a backend.",
                &["backends"],
                &[("name", "path")],
                Some(("DefaultArgsRequest", "application/json")),
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/backends/{name}/versions/{version}",
            delete_op_pp(
                "removeBackendVersion",
                "Remove a backend version",
                "Removes a specific version of a backend.",
                &["backends"],
                &[("name", "path"), ("version", "path")],
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/backends/check-updates",
            post_op(
                "checkBackendUpdates",
                "Check for backend updates",
                "Triggers a check for new versions of all backends.",
                &["backends"],
                None,
                None,
            ),
        ),
        (
            "/koji/v1/backends/{name}/versions",
            op_p(
                "get",
                "listBackendVersions",
                "List backend versions",
                "Returns all installed versions of a backend.",
                &["backends"],
                &[("name", "path")],
                Some("BackendVersion"),
            ),
        ),
        (
            "/koji/v1/backends/{name}/activate",
            post_op_p(
                "activateBackendVersion",
                "Activate a backend version",
                "Activates a specific version of a backend.",
                &["backends"],
                &[("name", "path")],
                Some(("ActivateRequest", "application/json")),
                Some("OkResponse"),
            ),
        ),
        // ── Jobs ────────────────────────────────────────────────────────────────
        (
            "/koji/v1/backends/jobs/{id}",
            op_p(
                "get",
                "getJob",
                "Get backend job status",
                "Returns the current status of a backend installation/update job.",
                &["jobs"],
                &[("id", "path")],
                Some("JobStatus"),
            ),
        ),
        (
            "/koji/v1/backends/jobs/{id}/events",
            op_p(
                "get",
                "jobEventsSse",
                "Stream job events (SSE)",
                "Server-sent events stream for real-time job progress and log lines.",
                &["jobs"],
                &[("id", "path")],
                None,
            ),
        ),
        // ── Updates ─────────────────────────────────────────────────────────────
        (
            "/koji/v1/updates",
            op(
                "get",
                "getUpdates",
                "Get cached update results",
                "Returns previously checked update status for backends and models.",
                &["updates"],
                None,
                Some("UpdatesListResponse"),
            ),
        ),
        (
            "/koji/v1/updates/check",
            post_op(
                "triggerUpdateCheck",
                "Trigger full update check",
                "Starts a new update check for all backends and models.",
                &["updates"],
                None,
                Some("CheckResponse"),
            ),
        ),
        (
            "/koji/v1/updates/check/{item_type}/{item_id}",
            post_op_pp(
                "checkSingleUpdate",
                "Check a single item",
                "Checks only one backend or model for available updates.",
                &["updates"],
                &[("item_type", "path"), ("item_id", "path")],
                None,
                Some("CheckResponse"),
            ),
        ),
        (
            "/koji/v1/updates/apply/backend/{name}",
            post_op_p(
                "applyBackendUpdate",
                "Apply backend update",
                "Triggers installation of the latest version for a backend.",
                &["updates"],
                &[("name", "path")],
                None,
                Some("JobResponse"),
            ),
        ),
        (
            "/koji/v1/updates/apply/model/{id}",
            post_op_p(
                "applyModelUpdate",
                "Apply model updates",
                "Enqueues selected quant downloads for a model through the download queue.",
                &["updates"],
                &[("id", "path")],
                Some(("ModelUpdateRequest", "application/json")),
                Some("ModelUpdateResponse"),
            ),
        ),
        // ── Downloads ───────────────────────────────────────────────────────────
        (
            "/koji/v1/downloads/active",
            op(
                "get",
                "getActiveDownloads",
                "Get active downloads",
                "Returns currently running and queued download jobs.",
                &["downloads"],
                None,
                Some("DownloadJob"),
            ),
        ),
        (
            "/koji/v1/downloads/history",
            op_q(
                "get",
                "getDownloadHistory",
                "Get download history",
                "Returns completed and failed download jobs with pagination.",
                &["downloads"],
                &[("limit", "query"), ("offset", "query")],
                Some("DownloadJob"),
            ),
        ),
        (
            "/koji/v1/downloads/{job_id}/cancel",
            post_op_p(
                "cancelDownload",
                "Cancel a download job",
                "Cancels a queued or active download. Does not affect completed jobs.",
                &["downloads"],
                &[("job_id", "path")],
                None,
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/downloads/events",
            op(
                "get",
                "downloadEventsSse",
                "Stream download events (SSE)",
                "Server-sent events stream for download lifecycle events.",
                &["downloads"],
                None,
                None,
            ),
        ),
        // ── Self-Update ─────────────────────────────────────────────────────────
        (
            "/koji/v1/self-update/check",
            op(
                "get",
                "checkSelfUpdate",
                "Check for self-update",
                "Checks if a newer version of the koji binary is available.",
                &["self-update"],
                None,
                Some("SelfUpdateCheck"),
            ),
        ),
        (
            "/koji/v1/self-update/update",
            post_op(
                "triggerSelfUpdate",
                "Trigger self-update",
                "Starts downloading and installing the latest koji binary.",
                &["self-update"],
                None,
                Some("SelfUpdateTrigger"),
            ),
        ),
        (
            "/koji/v1/self-update/events",
            op(
                "get",
                "selfUpdateEventsSse",
                "Stream self-update progress (SSE)",
                "Server-sent events stream showing self-update download and install progress.",
                &["self-update"],
                None,
                None,
            ),
        ),
        // ── Restore ─────────────────────────────────────────────────────────────
        (
            "/koji/v1/restore/preview",
            post_op(
                "restorePreview",
                "Preview restore archive",
                "Uploads a backup archive and returns its manifest for review before restoring.",
                &["restore"],
                Some(("RestorePreviewRequest", "multipart/form-data")),
                Some("RestorePreviewResponse"),
            ),
        ),
        (
            "/koji/v1/restore",
            post_op(
                "startRestore",
                "Start restore job",
                "Restores from a previously uploaded backup archive.",
                &["restore"],
                Some(("RestoreRequest", "application/json")),
                Some("JobResponse"),
            ),
        ),
        // ── Models (config CRUD) ────────────────────────────────────────────────
        (
            "/koji/v1/models",
            op2(
                "get",
                "listModels",
                "List all model configs",
                "Returns all model entries from config.toml plus available backends.",
                &["models"],
                None,
                Some("ModelsResponse"),
            ),
        ),
        (
            "/koji/v1/models",
            post_op(
                "createModel",
                "Create a new model config",
                "Adds a new `[models.<id>]` entry to config.toml.",
                &["models"],
                Some(("ModelBody", "application/json")),
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/models/{id}",
            op_p(
                "get",
                "getModel",
                "Get a model config",
                "Returns one model entry plus available backends.",
                &["models"],
                &[("id", "path")],
                Some("ModelConfig"),
            ),
        ),
        (
            "/koji/v1/models/{id}",
            put_op_p(
                "updateModel",
                "Update a model config",
                "Replaces the `[models.<id>]` entry in config.toml.",
                &["models"],
                &[("id", "path")],
                Some(("ModelBody", "application/json")),
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/models/{id}",
            delete_op_p(
                "deleteModel",
                "Delete a model config",
                "Removes the `[models.<id>]` entry from config.toml.",
                &["models"],
                &[("id", "path")],
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/models/{id}/rename",
            post_op_p(
                "renameModel",
                "Rename a model config",
                "Renames a model config entry by moving its key in config.toml.",
                &["models"],
                &[("id", "path")],
                Some(("RenameRequest", "application/json")),
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/models/{id}/refresh",
            post_op_p(
                "refreshModelMetadata",
                "Refresh model metadata",
                "Re-queries HuggingFace for the current commit hash of a model.",
                &["models"],
                &[("id", "path")],
                None,
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/models/{id}/verify",
            post_op_p(
                "verifyModelFiles",
                "Verify model files",
                "Recomputes SHA-256 checksums for all tracked files of a model.",
                &["models"],
                &[("id", "path")],
                None,
                Some("OkResponse"),
            ),
        ),
        (
            "/koji/v1/models/{id}/quants/{quant_key}",
            delete_op_pp(
                "deleteQuant",
                "Delete a quant file",
                "Deletes a specific quant file from disk and its config entry.",
                &["models"],
                &[("id", "path"), ("quant_key", "path")],
                Some("OkResponse"),
            ),
        ),
        // ── Benchmarks ──────────────────────────────────────────────────────────
        (
            "/koji/v1/benchmarks/run",
            post_op(
                "runBenchmark",
                "Run a benchmark",
                "Starts a new benchmark run against a model.",
                &["benchmarks"],
                Some(("BenchmarkRequest", "application/json")),
                Some("JobResponse"),
            ),
        ),
        (
            "/koji/v1/benchmarks/jobs/{id}",
            op_p(
                "get",
                "getBenchmarkResult",
                "Get benchmark result",
                "Returns the results of a completed benchmark run.",
                &["benchmarks"],
                &[("id", "path")],
                Some("BenchmarkResult"),
            ),
        ),
        (
            "/koji/v1/benchmarks/jobs/{id}/events",
            op_p(
                "get",
                "benchmarkEventsSse",
                "Stream benchmark events (SSE)",
                "Server-sent events stream for benchmark progress.",
                &["benchmarks"],
                &[("id", "path")],
                None,
            ),
        ),
        (
            "/koji/v1/benchmarks/history",
            op(
                "get",
                "listBenchmarkHistory",
                "List benchmark history",
                "Returns all completed and failed benchmark runs.",
                &["benchmarks"],
                None,
                Some("BenchmarkResult"),
            ),
        ),
        (
            "/koji/v1/benchmarks/history/{id}",
            delete_op_p(
                "deleteBenchmark",
                "Delete benchmark result",
                "Removes a benchmark result from history.",
                &["benchmarks"],
                &[("id", "path")],
                Some("OkResponse"),
            ),
        ),
        // ── Web API (logs, config, backup) ──────────────────────────────────────
        (
            "/koji/v1/logs",
            op_q(
                "get",
                "getLogs",
                "Get recent log lines",
                "Returns the last N lines of the koji.log file.",
                &["web-api"],
                &[("lines", "query")],
                None,
            ),
        ),
        (
            "/koji/v1/backup",
            op(
                "get",
                "createBackup",
                "Create backup archive",
                "Creates a tar.gz archive of config files and returns it as a download.",
                &["web-api"],
                None,
                None,
            ),
        ),
        (
            "/koji/v1/config",
            op(
                "get",
                "getConfig",
                "Get config file contents",
                "Returns the raw TOML content of the Koji config file.",
                &["web-api"],
                None,
                None,
            ),
        ),
        (
            "/koji/v1/config",
            post_op(
                "saveConfig",
                "Save config file contents",
                "Validates and saves the provided TOML as the Koji config file.",
                &["web-api"],
                Some(("ConfigBody", "application/json")),
                None,
            ),
        ),
        (
            "/koji/v1/config/structured",
            op(
                "get",
                "getStructuredConfig",
                "Get structured config",
                "Returns the parsed config as a JSON object with typed sections.",
                &["web-api"],
                None,
                None,
            ),
        ),
        (
            "/koji/v1/config/structured",
            post_op(
                "saveStructuredConfig",
                "Save structured config",
                "Validates and saves the provided JSON as the Koji config file.",
                &["web-api"],
                Some(("StructuredConfigBody", "application/json")),
                None,
            ),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Koji Web API",
            "description": "Endpoints served natively by the `koji-web` process (port 11435). All endpoints are prefixed with `/koji/v1/`.",
            "version": env!("CARGO_PKG_VERSION"),
            "license": {"name": "MIT"}
        },
        "servers": [{"url": "http://localhost:11435", "description": "Local Koji web UI (default port)"}],
        "tags": [
            {"name": "system", "description": "System health, capabilities, restart, and config reload"},
            {"name": "backends", "description": "Backend lifecycle — install, update, remove, versions, activate"},
            {"name": "jobs", "description": "Backend job status and SSE event streams"},
            {"name": "updates", "description": "Update checking and application for backends and models"},
            {"name": "downloads", "description": "Download queue management — active, history, cancel, events"},
            {"name": "self-update", "description": "Self-update check, trigger, and progress streaming"},
            {"name": "restore", "description": "Backup/restore archive preview and restoration"},
            {"name": "models", "description": "Model config CRUD — create, read, update, delete, rename, verify"},
            {"name": "benchmarks", "description": "Benchmark runs, results, and history"},
            {"name": "web-api", "description": "Log viewing, config editing, and backup download"}
        ],
        "paths": paths,
        "components": {
            "schemas": schemas(),
            "securitySchemes": {
                "csrf": {
                    "type": "apiKey",
                    "name": "X-CSRF-Token",
                    "in": "header",
                    "description": "CSRF double-submit token. GET requests return the token in Set-Cookie and X-CSRF-Token header; POST/PUT/PATCH must include it in both cookie and header."
                }
            }
        },
        "security": [{"csrf": []}]
    })
}

fn schemas() -> serde_json::Value {
    serde_json::json!({
        "OkResponse": {"type": "object", "required": ["ok"], "properties": {"ok": {"type": "boolean", "example": true}, "id": {"type": "string"}}},
        "ErrorResponse": {"type": "object", "required": ["error"], "properties": {"error": {"type": "string"}}},
        "JobResponse": {"type": "object", "required": ["job_id"], "properties": {"job_id": {"type": "string"}, "message": {"type": "string"}}},
        "CheckResponse": {"type": "object", "required": ["triggered", "message"], "properties": {"triggered": {"type": "boolean"}, "message": {"type": "string"}}},
        "BackendEntry": {"type": "object", "required": ["name", "backend_type", "version"], "properties": {"name": {"type": "string"}, "backend_type": {"type": "string"}, "version": {"type": "string"}, "is_active": {"type": "boolean"}}},
        "BackendVersion": {"type": "object", "required": ["version"], "properties": {"version": {"type": "string"}, "is_active": {"type": "boolean"}}},
        "InstallRequest": {"type": "object", "required": ["backend_type", "version"], "properties": {"backend_type": {"type": "string"}, "version": {"type": "string"}}},
        "UpdateRequest": {"type": "object", "required": ["backend_type"], "properties": {"backend_type": {"type": "string"}}},
        "DefaultArgsRequest": {"type": "object", "required": ["default_args"], "properties": {"default_args": {"type": "array", "items": {"type": "string"}}}},
        "ActivateRequest": {"type": "object", "required": ["version"], "properties": {"version": {"type": "string"}}},
        "JobStatus": {"type": "object", "required": ["id", "status"], "properties": {"id": {"type": "string"}, "status": {"type": "string"}, "progress": {"type": "number"}, "error_message": {"type": ["string", "null"]}}},
        "UpdatesListResponse": {"type": "object", "required": ["backends", "models"], "properties": {"backends": {"type": "array", "items": {"$ref": "#/components/schemas/UpdateCheckDto"}}, "models": {"type": "array", "items": {"$ref": "#/components/schemas/UpdateCheckDto"}}}},
        "UpdateCheckDto": {"type": "object", "required": ["item_type", "item_id", "status"], "properties": {"item_type": {"type": "string"}, "item_id": {"type": "string"}, "update_available": {"type": "boolean"}, "status": {"type": "string"}}},
        "ModelUpdateRequest": {"type": "object", "required": ["quants"], "properties": {"quants": {"type": "array", "items": {"type": "string"}}}},
        "ModelUpdateResponse": {"type": "object", "required": ["job_ids", "total"], "properties": {"job_ids": {"type": "array", "items": {"type": "string"}}, "total": {"type": "integer"}}},
        "DownloadJob": {"type": "object", "required": ["id", "status"], "properties": {"id": {"type": "string"}, "status": {"type": "string"}, "progress": {"type": "number"}, "speed_mbps": {"type": ["number", "null"]}}},
        "SelfUpdateCheck": {"type": "object", "required": ["current_version"], "properties": {"current_version": {"type": "string"}, "latest_version": {"type": ["string", "null"]}, "update_available": {"type": "boolean"}}},
        "SelfUpdateTrigger": {"type": "object", "required": ["triggered"], "properties": {"triggered": {"type": "boolean"}, "message": {"type": "string"}}},
        "RestorePreviewResponse": {"type": "object", "properties": {"archive_name": {"type": "string"}, "koji_version": {"type": "string"}}},
        "RestoreRequest": {"type": "object", "properties": {"upload_id": {"type": "string"}}},
        "RestorePreviewRequest": {"type": "object", "properties": {"file": {"type": "string", "format": "binary"}}},
        "Capabilities": {"type": "object", "properties": {"cuda_versions": {"type": "array", "items": {"type": "string"}}, "rocm_versions": {"type": "array", "items": {"type": "string"}}, "vulkan_support": {"type": "boolean"}}},
        "ModelConfig": {"type": "object", "required": ["id", "backend", "args", "enabled"], "properties": {"id": {"type": "string"}, "backend": {"type": "string"}, "model": {"type": ["string", "null"]}, "quant": {"type": ["string", "null"]}, "args": {"type": "array", "items": {"type": "string"}}, "enabled": {"type": "boolean"}}},
        "ModelBody": {"type": "object", "required": ["id", "backend"], "properties": {"id": {"type": "string"}, "backend": {"type": "string"}, "model": {"type": ["string", "null"]}, "quant": {"type": ["string", "null"]}, "args": {"type": "array", "items": {"type": "string"}}, "enabled": {"type": "boolean"}}},
        "ModelsResponse": {"type": "object", "required": ["models", "backends"], "properties": {"models": {"type": "array", "items": {"$ref": "#/components/schemas/ModelConfig"}}, "backends": {"type": "array", "items": {"type": "string"}}}},
        "RenameRequest": {"type": "object", "required": ["new_id"], "properties": {"new_id": {"type": "string"}}},
        "BenchmarkRequest": {"type": "object", "required": ["model_id"], "properties": {"model_id": {"type": "string"}, "quant": {"type": ["string", "null"]}}},
        "BenchmarkResult": {"type": "object", "required": ["id", "status"], "properties": {"id": {"type": "string"}, "model_id": {"type": "string"}, "status": {"type": "string"}, "results": {"type": ["object", "null"]}}},
        "ConfigBody": {"type": "object", "required": ["content"], "properties": {"content": {"type": "string"}}},
        "StructuredConfigBody": {"type": "object"}
    })
}

// ── Path item builders ────────────────────────────────────────────────────────

fn param(name: &str, loc: &str) -> serde_json::Value {
    serde_json::json!({"name": name, "in": loc, "required": true, "schema": {"type": "string"}})
}

fn op(
    _method: &str,
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn op_p(
    _method: &str,
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn op_q(
    _method: &str,
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert("parameters".into(), serde_json::json!(params.iter().map(|(n, _)| {
        serde_json::json!({"name": n, "in": "query", "required": false, "schema": {"type": "string"}})
    }).collect::<Vec<_>>()));
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn op2(
    _method: &str,
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn post_op(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    op("post", op_id, summary, desc, tags, request, response)
}

fn post_op_p(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn post_op_pp(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn put_op_p(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    request: Option<(&str, &str)>,
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some((schema, ct)) = request {
        item.insert(
            "requestBody".into(),
            serde_json::json!({"required": true, "content": {ct: {"schema": schema_ref(schema)}}}),
        );
    }
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn delete_op_p(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn delete_op_pp(
    op_id: &str,
    summary: &str,
    desc: &str,
    tags: &[&str],
    params: &[(&str, &str)],
    response: Option<&str>,
) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    item.insert("operationId".into(), op_id.into());
    item.insert("summary".into(), summary.into());
    item.insert("description".into(), desc.into());
    item.insert("tags".into(), serde_json::json!(tags));
    item.insert(
        "parameters".into(),
        serde_json::json!(params.iter().map(|(n, l)| param(n, l)).collect::<Vec<_>>()),
    );
    if let Some(r) = response {
        item.insert("responses".into(), responses_map([("200", r)]));
    } else {
        item.insert("responses".into(), responses_map([]));
    }
    serde_json::Value::Object(item)
}

fn schema_ref(name: &str) -> serde_json::Value {
    serde_json::json!({"$ref": format!("#/components/schemas/{}", name)})
}

fn responses_map<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (code, schema) in entries {
        let mut resp = serde_json::Map::new();
        resp.insert("description".into(), "Success".into());
        resp.insert(
            "content".into(),
            serde_json::json!({"application/json": {"schema": schema_ref(schema)}}),
        );
        map.insert(code.to_string(), serde_json::Value::Object(resp));
    }
    if map.is_empty() {
        let mut default = serde_json::Map::new();
        default.insert("description".into(), "Success".into());
        map.insert("200".to_string(), serde_json::Value::Object(default));
    }
    serde_json::Value::Object(map)
}

/// Serves the OpenAPI 3.1.0 specification as JSON at `GET /koji/v1/docs`.
#[cfg(feature = "ssr")]
pub async fn serve_spec() -> impl IntoResponse {
    let spec = spec();
    (StatusCode::OK, Json(spec)).into_response()
}
