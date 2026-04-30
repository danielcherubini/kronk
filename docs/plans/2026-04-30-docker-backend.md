# Docker Backend Plan

**Goal:** Add a new `BackendType::Docker` that lets users paste a docker-compose.yaml, have Tama manage the container lifecycle (start/stop/health/restart), and proxy requests to it.

**Architecture:** Docker backends use `docker compose up -d` and `docker compose down` via `tokio::process::Command`, mirroring how Tama already spawns local backends. The compose YAML is stored on disk in `<config>/docker/{server_name}/docker-compose.yaml` (source of truth) with a copy in the DB. Container name is injected into the YAML as `tama_{server_name}`. Models reference Docker backends via `backend = "docker"` + `docker_backend_name = "name"` in their config.

**Tech Stack:** Rust, tokio, reqwest, serde_yml (for YAML parsing — use the maintained fork, NOT serde_yaml), docker CLI, Leptos (web UI), SQLite.

---

### Task 1: Add BackendType::Docker, extend BackendInfo & ModelConfig & DB schema

**Context:**
This task adds the foundational data types and database schema changes needed to support Docker backends. It adds `Docker` to the `BackendType` enum, extends `BackendInfo` with `compose_yaml`, `dockerfile`, and `target_port` fields, extends `ModelConfig` with `tensor_parallel_size`, `docker_backend_name`, and `engine_type` fields, and updates the SQLite schema and all serialization/deserialization code. This is the first task because every other task depends on these types existing.

**Files:**
- Modify: `crates/tama-core/src/backends/registry/registry_ops.rs`
- Modify: `crates/tama-core/src/db/queries/backend_queries.rs` (NOT `queries.rs`)
- Modify: `crates/tama-core/src/db/migrations.rs` (add migration v19)
- Modify: `crates/tama-core/src/config/types.rs` (add `tensor_parallel_size`, `docker_backend_name`, `engine_type` to `ModelConfig`)
- Modify: `crates/tama-core/src/db/queries/types.rs` (add fields to `ModelConfigRecord`)
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs` (update upsert query)
- Modify: `crates/tama-web/src/api/backends/types.rs` (add `BackendType::Docker` to `job_to_active_dto()` match)
- Modify: `crates/tama-web/src/jobs.rs` (add `DockerInstall` to `JobKind` enum)
- Test: `crates/tama-core/src/backends/registry/registry_ops.rs` (add tests in existing `mod tests`)

**What to implement:**

1. **Add `Docker` variant to `BackendType` enum** in `registry_ops.rs`:
   ```rust
   pub enum BackendType {
       LlamaCpp,
       IkLlama,
       TtsKokoro,
       Docker,      // NEW
       Custom,
   }
   ```

2. **Update `Display` impl** for `BackendType`:
   ```rust
   BackendType::Docker => write!(f, "docker"),
   ```

3. **Update `FromStr` impl** for `BackendType`:
   ```rust
   "docker" => Ok(BackendType::Docker),
   ```

4. **Add fields to `BackendInfo` struct**:
   ```rust
   pub compose_yaml: Option<String>,
   pub dockerfile: Option<String>,
   pub target_port: Option<u16>,  // None for non-Docker backends
   ```
   Note: `target_port` is `Option<u16>` — existing non-Docker backends have `None`.

5. **Update `BackendInstallationRecord`** in `crates/tama-core/src/db/queries/backend_queries.rs` — add three new fields:
   ```rust
   pub compose_yaml: Option<String>,
   pub dockerfile: Option<String>,
   pub target_port: Option<i32>,  // SQLite INTEGER, nullable
   ```

6. **Update `insert_backend_installation`** SQL query in `backend_queries.rs` to include `compose_yaml`, `dockerfile`, `target_port` columns and update the params tuple. (Function signature doesn't change — it takes `&BackendInstallationRecord`.)

7. **Update ALL query functions** in `backend_queries.rs` that read from `backend_installations` to SELECT the new columns and update the `row.get(N)` mappings:
   - `get_active_backend`
   - `list_active_backends`
   - `list_backend_versions`
   - `get_backend_by_version`

8. **Update `record_to_backend_info`** to deserialize the new fields. Handle `Option<i32>` → `Option<u16>` conversion safely (return `None` for negative values).

9. **Update `backend_info_to_record`** to serialize the new fields. Handle `Option<u16>` → `Option<i32>` conversion.

10. **Update `BackendRegistry::add`** — it calls `backend_info_to_record` internally, so no change needed if we update that function.

11. **Update `BackendRegistry::update_version`** — it creates a new `BackendInfo` and calls `add`, so no change needed.

12. **Add fields to `ModelConfig` struct** in `config/types.rs`:
    ```rust
    pub tensor_parallel_size: Option<u32>,
    pub docker_backend_name: Option<String>,
    pub engine_type: Option<String>,  // "vllm", "llamacpp", etc. NOT the BackendType enum
    ```
    Note: `engine_type` is on the **model config**, not the backend registry. It tells Tama which set of config fields to use (vLLM args vs llama.cpp args) when building the container command. This is NOT the same as the `BackendType` enum — it's a free-form string for the inference engine name.

13. **Add fields to `ModelConfigRecord`** in `db/queries/types.rs`:
    ```rust
    pub tensor_parallel_size: Option<i32>,
    pub docker_backend_name: Option<String>,
    pub engine_type: Option<String>,
    ```

14. **Update `upsert_model_config`** query in `db/queries/model_config_queries.rs` to include the new columns.

15. **Add DB migration v19** in `crates/tama-core/src/db/migrations.rs` to add:
    - `compose_yaml TEXT DEFAULT NULL`
    - `dockerfile TEXT DEFAULT NULL`
    - `target_port INTEGER DEFAULT NULL`
    - `tensor_parallel_size INTEGER DEFAULT NULL`
    - `docker_backend_name TEXT DEFAULT NULL`
    - `engine_type TEXT DEFAULT NULL`
    Follow the exact pattern of existing migrations (v1–v18). The current latest migration is v18.

16. **Update `tama-web/src/api/backends/types.rs`** — add `BackendType::Docker` to the `job_to_active_dto()` match expression and to `KNOWN_BACKENDS`.

17. **Add `DockerInstall` to `JobKind` enum** in `crates/tama-web/src/jobs.rs`:
    ```rust
    pub enum JobKind {
        Install,
        Update,
        Restore,
        Benchmark,
        DockerInstall,  // NEW
    }
    ```
    Update `job_to_active_dto()` to handle `JobKind::DockerInstall`.

18. **Add tests** for the new `BackendType::Docker` serialization/deserialization round-trip.

**Steps:**
- [ ] Add `Docker` variant to `BackendType` enum and update `Display`/`FromStr` impls in `crates/tama-core/src/backends/registry/registry_ops.rs`
- [ ] Add `compose_yaml`, `dockerfile`, `target_port: Option<u16>` fields to `BackendInfo` struct
- [ ] Add corresponding fields to `BackendInstallationRecord` in `crates/tama-core/src/db/queries/backend_queries.rs`
- [ ] Update `insert_backend_installation` SQL query and params tuple in `backend_queries.rs`
- [ ] Update `get_active_backend`, `list_active_backends`, `list_backend_versions`, `get_backend_by_version` queries in `backend_queries.rs` to SELECT new columns and update `row.get(N)` mappings
- [ ] Update `record_to_backend_info` to deserialize new fields (handle `Option<i32>` → `Option<u16>`)
- [ ] Update `backend_info_to_record` to serialize new fields (handle `Option<u16>` → `Option<i32>`)
- [ ] Add `tensor_parallel_size: Option<u32>`, `docker_backend_name: Option<String>`, and `engine_type: Option<String>` to `ModelConfig` in `config/types.rs`
- [ ] Add corresponding fields to `ModelConfigRecord` in `db/queries/types.rs`
- [ ] Update `upsert_model_config` query in `db/queries/model_config_queries.rs`
- [ ] Add `serde_yml = "0.0.12"` to `[workspace.dependencies]` in root `Cargo.toml`, then add `serde_yml.workspace = true` to `tama-core/Cargo.toml`
- [ ] Add migration v19 in `crates/tama-core/src/db/migrations.rs` for all 6 new columns (3 on backend_installations + 3 on model_configs)
- [ ] Update `tama-web/src/api/backends/types.rs` — add `BackendType::Docker` to `job_to_active_dto()` match and `KNOWN_BACKENDS`
- [ ] Add `DockerInstall` variant to `JobKind` enum in `crates/tama-web/src/jobs.rs` and update `job_to_active_dto()`
- [ ] Run `cargo test --package tama-core -- backends::registry::tests` to verify existing tests still pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Commit with message: "feat(core): add BackendType::Docker with compose_yaml, dockerfile, target_port, ModelConfig fields"

**Acceptance criteria:**
- [ ] `BackendType::Docker` serializes to `"docker"` and deserializes from `"docker"`
- [ ] `BackendInfo` has `compose_yaml: Option<String>`, `dockerfile: Option<String>`, `target_port: Option<u16>` fields
- [ ] `BackendInstallationRecord` has corresponding DB columns (nullable)
- [ ] `ModelConfig` has `tensor_parallel_size: Option<u32>`, `docker_backend_name: Option<String>`, and `engine_type: Option<String>` fields
- [ ] Migration v19 adds all 6 new columns with NULL defaults
- [ ] `tama-web/src/api/backends/types.rs` compiles with `BackendType::Docker` in match
- [ ] `JobKind::DockerInstall` variant exists in `crates/tama-web/src/jobs.rs`
- [ ] All existing `BackendRegistry` tests still pass
- [ ] `cargo check --workspace` succeeds

---

### Task 2: Add BackendKind enum and extend ModelState

**Context:**
The existing `ModelState` variants store `backend_pid: u32` and the lifecycle code calls `kill_process(pid)` for every Ready model. Docker backends don't have a Linux PID — they have a container ID. This task adds a `BackendKind` enum (`Local | Docker`) to distinguish the two, adds `container_id: Option<String>` to all `ModelState` variants, and ensures the PID check in `check_idle_timeouts()` never runs for Docker backends. This task only adds fields and the enum — it does NOT add Docker-specific logic (that belongs in Task 4).

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle.rs`
- Test: `crates/tama-core/src/proxy/lifecycle.rs` (add tests in existing `mod tests`)

**What to implement:**

1. **Add `BackendKind` enum** in `crates/tama-core/src/proxy/types.rs`:
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum BackendKind {
       Local,
       Docker,
   }
   ```

2. **Add `backend_type: BackendKind` and `container_id: Option<String>`** to ALL `ModelState` variants:
   ```rust
   Starting {
       // ... existing fields ...
       backend_type: BackendKind,       // NEW
       container_id: Option<String>,    // NEW — always None in Starting (container not created yet)
   },
   Ready {
       // ... existing fields ...
       backend_pid: u32,           // 0 for Docker backends
       backend_type: BackendKind,  // NEW
       container_id: Option<String>, // Some for Docker, None for local
   },
   Unloading {
       // ... existing fields ...
       backend_pid: u32,           // 0 for Docker backends
       backend_type: BackendKind,  // NEW
       container_id: Option<String>, // Some for Docker, None for local
   },
   Failed { ... },  // No changes needed — Failed doesn't need backend_type
   ```

3. **Add accessor methods** to `ModelState`:
   ```rust
   impl ModelState {
       pub fn is_docker(&self) -> bool {
           match self {
               ModelState::Starting { backend_type, .. }
               | ModelState::Ready { backend_type, .. }
               | ModelState::Unloading { backend_type, .. } => *backend_type == BackendKind::Docker,
               ModelState::Failed { .. } => false,
           }
       }

       pub fn container_id(&self) -> Option<&str> {
           match self {
               ModelState::Starting { container_id, .. } => container_id.as_deref(),
               ModelState::Ready { container_id, .. } => container_id.as_deref(),
               ModelState::Unloading { container_id, .. } => container_id.as_deref(),
               ModelState::Failed { .. } => None,
           }
       }
   }
   ```

4. **Update `check_idle_timeouts()`** in `lifecycle.rs` — before calling `is_process_alive(pid)`, check `is_docker()`:
   ```rust
   if state.is_docker() {
       // Skip PID check for Docker backends — container state is checked separately
       continue;
   }
   // Existing PID check for local backends...
   ```
   Note: This is a NO-OP for now (just skips the PID check). The actual Docker container restart logic goes in Task 4.

5. **Update `unload_model()`** in `lifecycle.rs` — add a NO-OP Docker branch:
   ```rust
   if state.is_docker() {
       // Docker unload handled by docker::uninstall::stop_container() in Task 4
       models.remove(server_name);
       return Ok(());
   }
   // Existing PID kill logic for local backends...
   ```
   Note: This is a placeholder — the actual `docker compose down` call goes in Task 4.

6. **Update `evict_lru_if_needed()`** in `lifecycle.rs` — the existing code destructures `ModelState::Ready` and reconstructs `ModelState::Unloading`. Add `backend_type` and `container_id` to both the destructuring pattern and the reconstruction:
   ```rust
   if let ModelState::Ready {
       model_name,
       backend,
       backend_pid,
       backend_url,
       last_accessed,
       consecutive_failures,
       failure_timestamp,
       restart_count,
       load_time: _,
       backend_type,        // NEW
       container_id,        // NEW
   } = std::mem::take(state)
   {
       *state = ModelState::Unloading {
           model_name,
           backend,
           backend_pid,
           backend_url,
           last_accessed,
           consecutive_failures,
           failure_timestamp,
           restart_count,
           backend_type,        // NEW
           container_id,        // NEW
       };
   }
   ```

7. **Update all existing tests** that create `ModelState::Starting`, `ModelState::Ready`, or `ModelState::Unloading` to include the new fields (`backend_type: BackendKind::Local` for tests that don't involve Docker; `container_id: None`).

**Steps:**
- [ ] Add `BackendKind` enum to `crates/tama-core/src/proxy/types.rs`
- [ ] Add `backend_type: BackendKind` and `container_id: Option<String>` to `ModelState::Starting` (always None)
- [ ] Add `backend_type: BackendKind` and `container_id: Option<String>` to `ModelState::Ready`
- [ ] Add `backend_type: BackendKind` and `container_id: Option<String>` to `ModelState::Unloading`
- [ ] Add `is_docker()` helper method to `ModelState` (match on all variants)
- [ ] Add `container_id()` accessor method to `ModelState` (match on all variants)
- [ ] Update `check_idle_timeouts()` to skip `is_process_alive()` for `is_docker()` backends (NO-OP, logic in Task 4)
- [ ] Update `unload_model()` to skip `kill_process()` for `is_docker()` backends (NO-OP, logic in Task 4)
- [ ] Update `evict_lru_if_needed()` to include `backend_type` and `container_id` in Ready destructuring and Unloading construction
- [ ] Update all existing tests that construct `ModelState::Starting`, `ModelState::Ready`, or `ModelState::Unloading`
- [ ] Run `cargo test --package tama-core -- proxy::lifecycle::tests`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Commit with message: "feat(core): add BackendKind enum and container_id to ModelState for Docker backends"

**Acceptance criteria:**
- [ ] `BackendKind::Docker` and `BackendKind::Local` variants exist
- [ ] `ModelState::Starting` has `backend_type: BackendKind` and `container_id: Option<String>`
- [ ] `ModelState::Ready` has `backend_type: BackendKind` and `container_id: Option<String>`
- [ ] `ModelState::Unloading` has `backend_type: BackendKind` and `container_id: Option<String>`
- [ ] `is_docker()` returns correct value for all variants
- [ ] `container_id()` accessor returns the container ID for Ready/Unloading, None for Starting/Failed
- [ ] `check_idle_timeouts()` skips `is_process_alive()` for `is_docker()` backends
- [ ] `unload_model()` skips `kill_process()` for `is_docker()` backends (placeholder only)
- [ ] `evict_lru_if_needed()` compiles with new fields in Ready destructuring and Unloading construction
- [ ] All existing proxy lifecycle tests pass
- [ ] `cargo check --workspace` succeeds

---

### Task 3: Implement Docker backend module (install, uninstall, health, logs, templates, db)

**Context:**
This task creates the `docker/` module under `tama-core/src/backends/` with six files: `mod.rs`, `install.rs`, `uninstall.rs`, `health.rs`, `logs.rs`, `templates.rs`, and `db.rs`. These handle the core Docker lifecycle: starting containers, stopping them, checking health, streaming logs, providing built-in compose templates, and database access. This is the heart of the feature.

**Files:**
- Create: `crates/tama-core/src/backends/docker/mod.rs`
- Create: `crates/tama-core/src/backends/docker/install.rs`
- Create: `crates/tama-core/src/backends/docker/uninstall.rs`
- Create: `crates/tama-core/src/backends/docker/health.rs`
- Create: `crates/tama-core/src/backends/docker/logs.rs`
- Create: `crates/tama-core/src/backends/docker/templates.rs`
- Create: `crates/tama-core/src/backends/docker/db.rs`
- Modify: `crates/tama-core/src/backends/mod.rs` (add `pub mod docker;`)

**What to implement:**

1. **`docker/mod.rs`** — Module root with shared types:
   ```rust
   pub mod install;
   pub mod uninstall;
   pub mod health;
   pub mod logs;
   pub mod templates;
   pub mod db;

   pub struct DockerBackend {
       pub name: String,
       pub compose_yaml: String,
       pub dockerfile: Option<String>,
       pub target_port: Option<u16>,
       pub config_dir: std::path::PathBuf,
   }

   impl DockerBackend {
       pub fn compose_path(&self) -> std::path::PathBuf {
           self.config_dir.join("docker").join(&self.name).join("docker-compose.yaml")
       }
       pub fn dockerfile_path(&self) -> Option<std::path::PathBuf> {
           self.dockerfile.as_ref().map(|_| self.config_dir.join("docker").join(&self.name).join("Dockerfile"))
       }
       pub fn container_name(&self) -> String {
           format!("tama_{}", self.name)
       }
   }
   ```

2. **`docker/health.rs`** — Docker daemon availability check and container health:
   ```rust
   /// Check if Docker is available. Returns Ok(()) if `docker --version` succeeds.
   pub async fn check_docker_available() -> anyhow::Result<()>;

   /// Get container status via `docker inspect`.
   /// Returns "running", "exited", "dead", etc.
   pub async fn container_status(container_name: &str) -> anyhow::Result<String>;

   /// Get container ID via `docker ps`.
   /// Returns None if container not found.
   pub async fn container_id(container_name: &str) -> anyhow::Result<Option<String>>;
   ```

3. **`docker/install.rs`** — Start a container:
   ```rust
   pub async fn start_container(backend: &DockerBackend) -> anyhow::Result<String>;
   // Returns container_id
   // Steps:
   // 1. Create config_dir/docker/{name}/ directory
   // 2. Inject container_name into compose YAML:
   //    - Parse YAML with serde_yml
   //    - For the first service (or the only service), set `container_name: tama_{name}` and `network_mode: host`
   //    - If user already set `container_name` or `network_mode`, overwrite them
   //    - Serialize back to string
   // 3. Write compose YAML to disk
   // 4. If dockerfile exists, write it to disk
   // 5. Run `docker compose -f <path> up -d`
   // 6. Extract container_id from `docker ps --filter "name=tama_{name}" --format "{{.ID}}"`
   // 7. Return container_id
   ```

4. **`docker/uninstall.rs`** — Stop a container:
   ```rust
   pub async fn stop_container(backend: &DockerBackend) -> anyhow::Result<()>;
   // Steps:
   // 1. Run `docker compose -f <path> down -t 5`
   // 2. If container still running, run `docker kill <container_id>`
   // 3. Clean up disk files in config_dir/docker/{name}/
   ```

5. **`docker/logs.rs`** — Stream container logs:
   ```rust
   /// Stream container logs by running `docker logs -f <container_name>`.
   /// Returns a (Receiver, JoinHandle) tuple:
   /// - Receiver: for the SSE handler to read log lines from
   /// - JoinHandle: the spawned task running `docker logs -f`
   pub async fn stream_logs(container_name: &str) -> anyhow::Result<(
       tokio::sync::mpsc::Receiver<String>,
       tokio::task::JoinHandle<anyhow::Result<()>>,
   )>;
   ```
   Implementation: spawn a task that runs `docker logs -f <container_name>`, reads stdout line by line, and sends each line to the channel. The caller gets the Receiver to read from and the JoinHandle to await for completion.

   Note: Docker log streaming does NOT use the existing `BackendLogManager` broadcast system. It uses a separate SSE endpoint that directly streams `docker logs -f` output. This is simpler and avoids the complexity of integrating with the broadcast channel.

6. **`docker/templates.rs`** — Built-in compose templates:
   ```rust
   pub struct Template {
       pub name: &'static str,
       pub description: &'static str,
       pub default_port: u16,
       pub compose_yaml: &'static str,
   }

   /// Return the list of built-in templates.
   pub fn available_templates() -> &'static [Template];
   ```

   Include these templates:
   - **vLLM (ROCm/AITER)** — from the Reddit thread with env vars pre-configured
   - **vLLM (CUDA)** — standard NVIDIA vLLM
   - **llama.cpp** — official llama.cpp Docker image
   - **Custom** — blank template (empty string)

   vLLM ROCm/AITER template (with placeholders):
   ```yaml
   services:
     vllm:
       image: aml731/vllm-aiter:v0.19.1
       network_mode: host
       group_add:
         - video
       ipc: host
       cap_add:
         - SYS_PTRACE
       security_opt:
         - seccomp:unconfined
       devices:
         - /dev/kfd:/dev/kfd
         - /dev/dri:/dev/dri
       volumes:
         - {volume_path}:/data/models
       environment:
         - VLLM_ROCM_USE_AITER=1
         - VLLM_ROCM_ALLOW_RDNA4_AITER_ATTENTION=1
         - VLLM_ROCM_USE_AITER_UNIFIED_ATTENTION=1
         - VLLM_ROCM_USE_AITER_MHA=0
         - VLLM_ROCM_USE_AITER_PAGED_ATTN=0
         - VLLM_ROCM_USE_AITER_MOE=0
         - VLLM_ROCM_USE_AITER_LINEAR=0
         - FLASH_ATTENTION_TRITON_AMD_ENABLE=TRUE
         - PYTORCH_ALLOC_CONF=expandable_segments:True
       command: >
         python3 -m vllm.entrypoints.openai.api_server
         --model {model_path}
         --tensor-parallel-size {tp_size}
         --dtype auto
         --attention-backend ROCM_AITER_UNIFIED_ATTN
         --max-model-len 131072
         --gpu-memory-utilization 0.95
         --enable-prefix-caching
         --trust-remote-code
         --quantization fp8
         --host 0.0.0.0
         --port 8000
   ```

7. **Compose YAML injection** — in `install.rs`, before writing the YAML:
   - Parse the YAML with `serde_yml`
   - For each service, set `container_name: tama_{name}` and `network_mode: host`
   - Serialize back to string
   - This ensures the container name is always `tama_{server_name}` regardless of what the user wrote

8. **`docker/db.rs`** — Database helpers:
   ```rust
   /// Look up a Docker backend by name from the DB.
   /// Requires the DB directory path to open the connection.
   pub async fn get_backend_by_name(name: &str, db_dir: &std::path::Path) -> anyhow::Result<Option<DockerBackend>>;
   ```

**Steps:**
- [ ] Create `crates/tama-core/src/backends/docker/mod.rs` with module declarations and `DockerBackend` struct
- [ ] Create `crates/tama-core/src/backends/docker/health.rs` with `check_docker_available()`, `container_status()`, `container_id()`
- [ ] Create `crates/tama-core/src/backends/docker/install.rs` with `start_container()` that injects container_name into compose YAML
- [ ] Create `crates/tama-core/src/backends/docker/uninstall.rs` with `stop_container()`
- [ ] Create `crates/tama-core/src/backends/docker/logs.rs` with `stream_logs()` using `docker logs -f` (returns Receiver + JoinHandle, separate SSE, not BackendLogManager)
- [ ] Create `crates/tama-core/src/backends/docker/templates.rs` with built-in templates (vLLM ROCm, vLLM CUDA, llama.cpp, Custom)
- [ ] Create `crates/tama-core/src/backends/docker/db.rs` with `get_backend_by_name()` function
- [ ] Modify `crates/tama-core/src/backends/mod.rs` to add `pub mod docker;`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Commit with message: "feat(core): implement Docker backend module with install, uninstall, health, logs, templates, and db"

**Acceptance criteria:**
- [ ] `DockerBackend` struct has `name`, `compose_yaml`, `dockerfile`, `target_port: Option<u16>`, `config_dir`
- [ ] `container_name()` returns `"tama_{name}"`
- [ ] `check_docker_available()` returns `Ok(())` when Docker CLI is installed
- [ ] `start_container()` writes compose YAML with injected `container_name` and runs `docker compose up -d`
- [ ] `stop_container()` runs `docker compose down -t 5` and cleans up disk files
- [ ] `stream_logs()` runs `docker logs -f` and returns a Receiver + JoinHandle
- [ ] `available_templates()` returns 4 templates (vLLM ROCm, vLLM CUDA, llama.cpp, Custom)
- [ ] `get_backend_by_name()` queries the DB for a Docker backend by name
- [ ] `cargo check --workspace` succeeds

---

### Task 4: Integrate Docker backend into proxy lifecycle

**Context:**
The existing `load_model()` in `lifecycle.rs` spawns local processes via `tokio::process::Command`. For Docker backends, we need a completely different path. This task extracts the local backend logic into `load_local_backend()` and adds `load_docker_backend()` that calls the Docker module from Task 3. It also updates `check_idle_timeouts()` to handle Docker container restarts and updates `unload_model()` to call `docker compose down` for Docker backends.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`
- Modify: `crates/tama-core/src/config/loader.rs` (add `docker_dir()` method — NOT `mod.rs`)
- Test: `crates/tama-core/src/proxy/lifecycle.rs` (add tests)

**What to implement:**

1. **Add `docker_dir()` to `Config`** in `crates/tama-core/src/config/loader.rs` (NOT `mod.rs` — all Config impl blocks with methods are in `loader.rs`):
   ```rust
   impl Config {
       pub fn docker_dir(&self) -> Result<PathBuf> {
           let base = Config::base_dir()?;
           let dir = base.join("docker");
           std::fs::create_dir_all(&dir)?;
           Ok(dir)
       }
   }
   ```

2. **Extract local backend logic** — rename `load_model()` to `load_local_backend()` and keep the existing logic intact. This is a pure refactor with no behavior change.

3. **Add `load_docker_backend()`** — new method in `ProxyState`:
   ```rust
   pub async fn load_docker_backend(
       &self,
       model_name: &str,
       server_name: &str,
       server_config: &crate::config::ServerConfig,
       backend_config: &crate::config::BackendConfig,
   ) -> Result<String>
   ```
   Steps:
   a. Look up the Docker backend by `docker_backend_name` from the model config (via `docker::db::get_backend_by_name()`)
   b. Replace placeholders in the compose YAML:
      - `{volume_path}` → `<config>/models/{model_name}` (host path)
      - `{model_path}` → same as `{volume_path}`
      - `{tp_size}` → `model_config.tensor_parallel_size.unwrap_or(1)`
   c. Create a `DockerBackend` struct with the templated YAML
   d. Call `docker::install::start_container()` to start the container
   e. Wait for health check (poll `http://localhost:{target_port}/health` every 500ms, timeout `config.proxy.startup_timeout_secs`). Respect `model_config.health_check.url` if set — use it as the path instead of `/health`.
   f. Get container_id via `docker::health::container_id()`
   g. Transition model state to `Ready` with `backend_type: BackendKind::Docker`, `backend_pid: 0`, `container_id: Some(id)`
   h. Write to DB (same as existing `insert_active_model`)

4. **Update `load_model()`** — the entry point that dispatches:
   ```rust
   pub async fn load_model(&self, model_name: &str, ...) -> Result<String> {
       // ... existing setup to resolve server_name ...
       let backend_type = resolve_backend_type(&server_config.backend, &db_conn)?;
       match backend_type {
           BackendKind::Docker => self.load_docker_backend(...).await,
           BackendKind::Local => self.load_local_backend(...).await,
       }
   }
   ```

5. **Add `resolve_backend_type()` helper** in `lifecycle.rs`:
   ```rust
   fn resolve_backend_type(backend_name: &str, conn: &rusqlite::Connection) -> BackendKind {
       if let Ok(Some(record)) = get_active_backend(conn, backend_name) {
           if record.backend_type == "docker" {
               return BackendKind::Docker;
           }
       }
       BackendKind::Local  // Default: all non-Docker backends are Local
   }
   ```

6. **Update `unload_model()`** — add Docker branch (replace the NO-OP from Task 2):
   ```rust
   if state.is_docker() {
       // Look up the Docker backend name from model configs
       let model_configs = self.model_configs.read().await;
       let docker_name = model_configs.values()
           .find_map(|mc| mc.docker_backend_name.as_ref())
           .ok_or_else(|| anyhow!("No docker_backend_name found"))?;
       drop(model_configs);
       let db_dir = self.db_dir.clone().ok_or_else(|| anyhow!("DB dir not configured"))?;
       let docker_backend = docker::db::get_backend_by_name(docker_name, &db_dir).await?
           .ok_or_else(|| anyhow!("Docker backend '{}' not found", docker_name))?;
       docker::uninstall::stop_container(&docker_backend).await?;
       models.remove(server_name);
       return Ok(());
   }
   // Existing local backend logic...
   ```

7. **Update `check_idle_timeouts()`** — add Docker container restart logic (replace the NO-OP from Task 2):
   ```rust
   if state.is_docker() {
       if let Some(ref container_id) = state.container_id() {
           let status = docker::health::container_status(container_id).await.ok();
           match status.as_deref() {
               Some("exited" | "dead") => {
                   if state.restart_count() < max_restarts {
                       // Spawn restart task
                       tokio::spawn(async move { ... });
                   } else {
                       models.insert(server_name, ModelState::Failed { ... });
                   }
               }
               Some("running") => {
                   // Health check the HTTP endpoint
                   if !super::process::check_health(&backend_url, Some(5)).await.is_ok_and(|r| r.status().is_success()) {
                       // Treat as dead — same as PID check failure for local
                   }
               }
               _ => {}
           }
       }
       continue;
   }
   // Existing local backend logic...
   ```

8. **Always perform at least one health check** for already-running containers in `load_docker_backend()`. If the container is running but the health endpoint fails, treat it as a new start (go to `docker compose down` then `docker compose up`).

**Steps:**
- [ ] Add `docker_dir()` method to `Config` in `crates/tama-core/src/config/loader.rs`
- [ ] Extract `load_model()` to `load_local_backend()` (pure refactor, no behavior change)
- [ ] Add `resolve_backend_type()` helper in `lifecycle.rs`
- [ ] Add `load_docker_backend()` method with compose YAML templating and container start
- [ ] Update `load_model()` to dispatch to `load_local_backend()` or `load_docker_backend()`
- [ ] Update `unload_model()` to call `docker::uninstall::stop_container()` for Docker backends (look up docker_backend_name from model configs)
- [ ] Update `check_idle_timeouts()` to check Docker container status via `docker inspect` and restart if needed
- [ ] Always perform at least one health check for already-running containers
- [ ] Respect `model_config.health_check.url` for custom health endpoint paths
- [ ] Run `cargo test --package tama-core -- proxy::lifecycle::tests`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Commit with message: "feat(core): integrate Docker backend into proxy lifecycle with load_docker_backend()"

**Acceptance criteria:**
- [ ] `load_model()` dispatches to `load_local_backend()` or `load_docker_backend()` based on backend type
- [ ] `load_docker_backend()` starts a container, waits for health check, and transitions to Ready
- [ ] `unload_model()` calls `docker compose down` for Docker backends (looks up docker_backend_name from model configs)
- [ ] `check_idle_timeouts()` restarts exited Docker containers (respecting max_restarts)
- [ ] Placeholder templating replaces `{volume_path}`, `{model_path}`, `{tp_size}` in compose YAML
- [ ] At least one health check is always performed (even for already-running containers)
- [ ] Custom health check URL from `model_config.health_check.url` is respected
- [ ] All existing proxy tests pass
- [ ] `cargo check --workspace` succeeds

---

### Task 5: Add Docker backend API endpoints in tama-web

**Context:**
This task adds the REST API endpoints under `/tama/v1/backends/docker/` that the web UI calls. The endpoints live in `tama-web/src/api/backends/` (NOT `tama-core/src/proxy/handlers/`) because the existing backend install/update/delete/list routes are in `tama-web`. The handlers call into `tama-core`'s Docker module for the actual container operations. This task also adds the SSE streaming endpoints for install progress and logs.

**Files:**
- Create: `crates/tama-web/src/api/backends/docker.rs`
- Modify: `crates/tama-web/src/api/backends/mod.rs`
- Modify: `crates/tama-web/src/server.rs` (register routes in `build_router()`)
- Test: `crates/tama-web/src/api/backends/docker.rs` (add tests if test infrastructure exists)

**What to implement:**

1. **API endpoint definitions** in `server.rs` `build_router()`:
   ```rust
   // Under the existing backend routes section:
   .route("/tama/v1/backends/docker/install", post(handle_docker_install))
   .route("/tama/v1/backends/docker/install/:job_id/stream", get(handle_docker_install_stream))
   .route("/tama/v1/backends/docker/:name", delete(handle_docker_uninstall))
   .route("/tama/v1/backends/docker/:name/logs", get(handle_docker_logs))
   .route("/tama/v1/backends/docker/:name/status", get(handle_docker_status))
   ```

2. **`handle_docker_install`** — POST handler:
   - Request body: `DockerInstallRequest { name, compose_yaml, dockerfile, target_port, version }`
   - Creates a job ID and spawns an async task that:
     a. Validates the compose YAML (parse with serde_yml)
     b. Creates the Docker backend record in the DB
     c. Calls `tama_core::backends::docker::install::start_container()` to start the container
     d. Broadcasts progress events via the job stream
   - Returns: `{ job_id: "abc-123" }`

3. **`handle_docker_install_stream`** — SSE stream for install progress:
   - Events: `log` (with level and message), `status` (with container_id and state)
   - Reuses the existing job streaming infrastructure (same pattern as pull jobs, `JobKind::DockerInstall`)

4. **`handle_docker_uninstall`** — DELETE handler:
   - Looks up the Docker backend by name from the DB
   - Calls `tama_core::backends::docker::uninstall::stop_container()`
   - Removes the backend from the DB registry
   - Returns 200 OK

5. **`handle_docker_logs`** — GET handler:
   - Gets the container name (`tama_{name}`)
   - Calls `tama_core::backends::docker::logs::stream_logs()`
   - Reads from the Receiver and converts each line to an SSE event
   - Returns SSE stream of log lines (directly, not through BackendLogManager)

6. **`handle_docker_status`** — GET handler:
   - Gets the container name
   - Calls `tama_core::backends::docker::health::container_status()` and `container_id()`
   - Returns: `{ state, container_id, port, health_url, uptime_seconds, exit_code }`

7. **DTO types** in `docker.rs`:
   ```rust
   #[derive(Deserialize)]
   pub struct DockerInstallRequest {
       pub name: String,
       pub compose_yaml: String,
       pub dockerfile: Option<String>,
       pub target_port: Option<u16>,
       pub version: Option<String>,
   }

   #[derive(Serialize)]
   pub struct DockerInstallResponse {
       pub job_id: String,
   }

   #[derive(Serialize)]
   pub struct DockerStatusResponse {
       pub state: String,
       pub container_id: Option<String>,
       pub port: Option<u16>,
       pub health_url: String,
       pub uptime_seconds: Option<u64>,
       pub exit_code: Option<i32>,
   }
   ```

**Note on port conflict detection:** Do NOT implement port conflict detection via `ss -tlnp`. It's fragile (requires `ss`, not available on macOS, requires root). Instead, let Docker fail naturally with its own error message, and surface that error to the user.

**Steps:**
- [ ] Define DTO types (`DockerInstallRequest`, `DockerInstallResponse`, `DockerStatusResponse`) in `crates/tama-web/src/api/backends/docker.rs`
- [ ] Implement `handle_docker_install` with async job spawning and progress streaming (JobKind::DockerInstall)
- [ ] Implement `handle_docker_install_stream` SSE endpoint
- [ ] Implement `handle_docker_uninstall` DELETE handler
- [ ] Implement `handle_docker_logs` SSE handler (reads from Receiver, converts to SSE events)
- [ ] Implement `handle_docker_status` GET handler
- [ ] Register all routes in `crates/tama-web/src/server.rs` `build_router()`
- [ ] Export handlers in `crates/tama-web/src/api/backends/mod.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Commit with message: "feat(web): add Docker backend API endpoints (install, uninstall, logs, status)"

**Acceptance criteria:**
- [ ] `POST /tama/v1/backends/docker/install` accepts compose YAML and returns a job_id
- [ ] `GET /tama/v1/backends/docker/install/:job_id/stream` returns SSE progress events
- [ ] `DELETE /tama/v1/backends/docker/:name` stops the container and removes from DB
- [ ] `GET /tama/v1/backends/docker/:name/logs` streams container logs via SSE
- [ ] `GET /tama/v1/backends/docker/:name/status` returns container state
- [ ] No port conflict detection (Docker handles failures naturally)
- [ ] `cargo check --workspace` succeeds

---

### Task 6: Add web UI components (Docker install modal, template picker)

**Context:**
This task adds the web UI components for the Docker backend feature. It includes an "Add Docker" button on the backends page, a modal with template picker and custom YAML editor, and the wiring to call the API endpoints from Task 5. This is a Leptos WASM component.

**Files:**
- Create: `crates/tama-web/src/components/docker_install_modal.rs`
- Create: `crates/tama-web/src/components/docker_template_card.rs`
- Modify: `crates/tama-web/src/pages/backends.rs`
- Modify: `crates/tama-web/src/components/mod.rs`
- Test: `crates/tama-web/src/components/docker_install_modal.rs` (add tests if Leptos testing is set up)

**What to implement:**

1. **`docker_template_card.rs`** — A card component for template selection:
   ```rust
   #[component]
   pub fn DockerTemplateCard(
       template: Template,
       on_select: Callback<()>,
   ) -> impl IntoView {
       view! {
           <div class="template-card" on:click=move |_| on_select.emit(())>
               <h3>{template.name}</h3>
               <p>{template.description}</p>
               <span>"Port: " {template.default_port}</span>
           </div>
       }
   }
   ```

2. **`docker_install_modal.rs`** — The install modal with two tabs:
   - **Template tab**: Grid of template cards. Clicking a card loads the template YAML into the editor.
   - **Custom tab**: Two textareas (compose YAML + Dockerfile), port input, validate button, install button.

   State:
   ```rust
   let active_tab = RwSignal::new("template"); // "template" | "custom"
   let compose_yaml = RwSignal::new(String::new());
   let dockerfile = RwSignal::new(String::new());
   let target_port = RwSignal::new(Option::<u16>::None);
   let backend_name = RwSignal::new(String::new());
   let error = RwSignal::new(Option::<String>::None);
   let installing = RwSignal::new(false);
   let install_job_id = RwSignal::new(Option::<String>::None);
   ```

3. **Backends page changes** in `backends.rs`:
   - Add "Add Docker" button in the header area (next to existing backend buttons)
   - When clicked, set `install_modal_for` to `Some("docker")`
   - Render the `DockerInstallModal` when `install_modal_for` is `Some("docker")`
   - Note: The existing `InstallModal` component is for local backends (llama_cpp, etc.). The Docker modal is a completely separate component with different fields.

4. **Install flow** in the modal:
   - User fills in name, selects template or writes YAML, sets port
   - Clicks "Install" → POST to `/tama/v1/backends/docker/install`
   - Shows a progress indicator (SSE stream from `/tama/v1/backends/docker/install/:job_id/stream`)
   - On success, refresh the backends list

5. **YAML validation** — use `serde_yml::from_str::<serde_yml::Value>(&compose_yaml)` to check syntax. Show error if invalid.

6. **Template selection** — when a template card is clicked, populate the compose YAML textarea with the template's YAML.

**Steps:**
- [ ] Create `crates/tama-web/src/components/docker_template_card.rs` with `DockerTemplateCard` component
- [ ] Create `crates/tama-web/src/components/docker_install_modal.rs` with template tab, custom tab, YAML editor, install button
- [ ] Export components in `crates/tama-web/src/components/mod.rs`
- [ ] Modify `crates/tama-web/src/pages/backends.rs` to add "Add Docker" button
- [ ] Wire install button to POST `/tama/v1/backends/docker/install`
- [ ] Wire install progress to SSE stream from `/tama/v1/backends/docker/install/:job_id/stream`
- [ ] Add YAML validation using `serde_yml::from_str`
- [ ] Add error handling and loading states
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Commit with message: "feat(web): add Docker install modal with template picker and YAML editor"

**Acceptance criteria:**
- [ ] "Add Docker" button appears on the backends page
- [ ] Modal opens with two tabs: Template and Custom
- [ ] Template cards display vLLM ROCm/AITER, vLLM CUDA, llama.cpp, Custom
- [ ] Clicking a template card populates the YAML editor
- [ ] YAML validation shows error for invalid YAML
- [ ] Install button POSTs to the API and shows progress via SSE
- [ ] On success, backends list refreshes
- [ ] `cargo check --workspace` succeeds

---

### Task 7: Add Docker availability check banner and CLI integration

**Context:**
This task adds the Docker availability check at proxy startup (with a UI banner if Docker is unavailable), updates the CLI backend commands to handle Docker backends, and ensures `safe_remove_installation` is guarded for Docker backends. This is the final polish task.

**Files:**
- Modify: `crates/tama-core/src/proxy/server/mod.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`
- Modify: `crates/tama-core/src/backends/mod.rs` (safe_remove_installation guard)
- Modify: `crates/tama-cli/src/commands/backend/mod.rs` (guard safe_remove_installation)
- Modify: `crates/tama-web/src/pages/backends.rs` (Docker unavailable banner)

**What to implement:**

1. **Initialize `docker_available` in `ProxyState::new()`** (sync function) in `crates/tama-core/src/proxy/state.rs`:
   ```rust
   pub fn new(config: Config, db_dir: Option<PathBuf>) -> Self {
       // ... existing fields ...
       docker_available: Arc::new(tokio::sync::RwLock::new(false)),  // Default: false, updated in ProxyServer::new()
   }
   ```

2. **Docker availability check at startup** in `proxy/server/mod.rs`:
   ```rust
   // In ProxyServer::new():
   let docker_available = docker::health::check_docker_available().await.is_ok();
   if !docker_available {
       tracing::warn!("Docker daemon not available — Docker backends will not function");
   }
   *state.docker_available.write().await = docker_available;
   ```

3. **Docker unavailable banner** in the web UI backends page:
   - If `docker_available` is false, show a warning banner at the top of the backends page
   - Message: "Docker is not installed or not running. Docker backends will not function."

4. **Guard `safe_remove_installation`** in `crates/tama-core/src/backends/mod.rs`:
   The existing `safe_remove_installation` validates that the path is within `backends_dir()`. Docker backends are stored in `<config>/docker/` which is OUTSIDE `backends_dir()`, so `safe_remove_installation` will already reject them with "path is outside the managed backends directory". However, we should explicitly guard against calling it for Docker backends to avoid confusion:
   ```rust
   // In the CLI and web API delete handlers, before calling safe_remove_installation:
   if info.backend_type != BackendType::Docker {
       safe_remove_installation(&info)?;
   } else {
       // Docker backends: clean up compose files directly (no safe_remove_installation)
       let docker_dir = config.docker_dir()?.join(&info.name);
       if docker_dir.exists() {
           std::fs::remove_dir_all(&docker_dir)?;
       }
   }
   ```

5. **Add `docker_available` to system capabilities** — expose via `/tama/v1/system/capabilities` endpoint so the web UI can check it.

**Steps:**
- [ ] Add `docker_available: Arc<RwLock<bool>>` field to `ProxyState` and initialize to `false` in `ProxyState::new()`
- [ ] Add Docker availability check at proxy startup in `proxy/server/mod.rs`
- [ ] Guard `safe_remove_installation` calls in CLI and web API for Docker backends
- [ ] Add Docker unavailable banner to backends page in web UI
- [ ] Expose `docker_available` in system capabilities endpoint
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Commit with message: "feat(core): add Docker availability check, CLI integration, and UI banner"

**Acceptance criteria:**
- [ ] Proxy checks `docker --version` at startup and logs warning if unavailable
- [ ] Web UI shows banner when Docker is unavailable
- [ ] `safe_remove_installation` is skipped for Docker backends (defensive guard)
- [ ] Docker backend removal cleans up `<config>/docker/{name}/` directory
- [ ] `docker_available` is exposed in system capabilities
- [ ] `cargo check --workspace` succeeds

---

## Task Dependencies

| Task | Depends On |
|------|-----------|
| Task 1 | None |
| Task 2 | Task 1 |
| Task 3 | Task 1 |
| Task 4 | Task 2, Task 3 |
| Task 5 | Task 3 |
| Task 6 | Task 5 |
| Task 7 | Task 1, Task 2, Task 4, Task 5, Task 6 |

## Total Estimated Tasks: 7

## Notes for Implementing Agent

- The existing codebase uses `tokio::process::Command` for spawning local backends — use the same pattern for Docker CLI commands
- All Docker commands should use `docker compose` (not `docker-compose`, which is the legacy standalone command)
- Use `serde_yml` (the maintained fork of `serde_yaml`) for YAML parsing — add to `[workspace.dependencies]` in root `Cargo.toml`, then `serde_yml.workspace = true` in `tama-core/Cargo.toml`
- The DB migration system uses SQLite — follow the pattern from existing migrations (e.g., `2026-03-30-sqlite-db-and-model-update.md`). Migration v19 adds 6 new columns.
- The backend queries are in `crates/tama-core/src/db/queries/backend_queries.rs` (NOT `queries.rs`)
- For the SSE log streaming, use a direct `docker logs -f` pipe — do NOT integrate with the existing `BackendLogManager` broadcast system. Docker logs have their own separate SSE endpoint. The `stream_logs()` function returns a `(Receiver, JoinHandle)` tuple.
- The job streaming for install progress follows the same pattern as pull jobs (`tama/v1/pulls/:job_id/stream`)
- Template YAML uses `{volume_path}`, `{model_path}`, `{tp_size}` as placeholders — these are replaced in `load_docker_backend()` before writing to disk
- `{volume_path}` and `{model_path}` resolve to the same value (`<config>/models/{model_name}`). `{volume_path}` is used in the volume mount spec, `{model_path}` is used in the `--model` CLI flag.
- The API endpoints go in `tama-web/src/api/backends/`, NOT `tama-core/src/proxy/handlers/`. The handlers call into `tama-core`'s Docker module.
- Port conflict detection is NOT implemented — let Docker fail naturally and surface its error message.
- `safe_remove_installation` already rejects Docker backends because their path is outside `backends_dir()` — the guard is defensive.
- The existing `InstallModal` component (for local backends) is separate from the new `DockerInstallModal`. They are independent components.
- `ModelConfig.engine_type` (e.g., "vllm", "llamacpp") distinguishes the inference engine for Docker backends. This is NOT the `BackendType` enum — it's a free-form string.
- `Config.docker_dir()` is in `config/loader.rs` (NOT `config/mod.rs`) — all Config impl blocks with methods are in `loader.rs`.
- `JobKind::DockerInstall` is added in Task 1 to `crates/tama-web/src/jobs.rs`.
- `ModelState.container_id()` is always `None` in the `Starting` variant (container not created yet).
- `ProxyState::new()` is sync — `docker_available` is initialized to `false` there, then updated in the async `ProxyServer::new()`.
- `evict_lru_if_needed()` must be updated in Task 2 to include `backend_type` and `container_id` in its destructuring/reconstruction patterns.
- `list_backend_versions` and `get_backend_by_version` queries in `backend_queries.rs` must be updated in Task 1 to SELECT the new columns.
