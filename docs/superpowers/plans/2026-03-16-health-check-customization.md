# Health Check Customization Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow per-profile health check URLs, intervals, timeouts, and retry counts instead of the current single URL on BackendConfig and global interval on Supervisor.

**Architecture:** Add a `HealthCheck` struct with `url`, `interval_ms`, `timeout_ms`, and `retries` fields. Add an optional `health_check` field to `ProfileConfig` that overrides the backend/supervisor defaults. `ProcessSupervisor` accepts these resolved values. The config merges profile → backend → supervisor defaults.

**Tech Stack:** Rust, serde/toml, existing `ProcessSupervisor`, reqwest

---

## File Structure

### Files to modify

| File | Changes |
|------|---------|
| `crates/kronk-core/src/config.rs` | Add `HealthCheck` struct, add `health_check` field to `ProfileConfig`, add `resolve_health_check` method |
| `crates/kronk-core/src/process.rs` | Update `ProcessSupervisor::new` to accept `HealthCheck` config |
| `crates/kronk-cli/src/main.rs` | Update `cmd_run` and service install to pass resolved health check config |

---

## Chunk 1: HealthCheck Config Type

### Task 1: Define `HealthCheck` struct

**Files:**
- Modify: `crates/kronk-core/src/config.rs`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_health_check_roundtrip() {
    let toml_str = r#"
backend = "llama_cpp"
args = []

[health_check]
url = "http://localhost:9090/health"
interval_ms = 3000
timeout_ms = 5000
retries = 3
"#;
    let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
    let hc = profile.health_check.unwrap();
    assert_eq!(hc.url, Some("http://localhost:9090/health".to_string()));
    assert_eq!(hc.interval_ms, Some(3000));
    assert_eq!(hc.timeout_ms, Some(5000));
    assert_eq!(hc.retries, Some(3));
}

#[test]
fn test_profile_without_health_check_still_works() {
    let toml_str = r#"
backend = "llama_cpp"
args = []
"#;
    let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
    assert!(profile.health_check.is_none());
}
```

- [ ] **Step 2: Add `HealthCheck` struct and field**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HealthCheck {
    /// Health check endpoint URL. Overrides backend's health_check_url.
    #[serde(default)]
    pub url: Option<String>,
    /// Polling interval in milliseconds. Overrides supervisor.health_check_interval_ms.
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// HTTP timeout in milliseconds per health check request (default: 3000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Number of consecutive failures before declaring unhealthy (default: 1).
    #[serde(default)]
    pub retries: Option<u32>,
}
```

Add to `ProfileConfig`:

```rust
    /// Per-profile health check overrides.
    #[serde(default)]
    pub health_check: Option<HealthCheck>,
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p kronk-core -- config`
Expected: All tests PASS (fix any `ProfileConfig` literal constructors that need `health_check: None`)

- [ ] **Step 4: Commit**

```bash
git add crates/kronk-core/src/config.rs
git commit -m "feat: add HealthCheck config struct with per-profile overrides"
```

### Task 2: Add `resolve_health_check` method

**Files:**
- Modify: `crates/kronk-core/src/config.rs`

- [ ] **Step 1: Write test**

```rust
#[test]
fn test_resolve_health_check_defaults() {
    let config = Config::default();
    let profile = config.profiles.get("default").unwrap();
    let hc = config.resolve_health_check(profile);
    assert_eq!(hc.url, Some("http://localhost:8080/health".to_string()));
    assert_eq!(hc.interval_ms, Some(5000)); // from supervisor default
    assert_eq!(hc.timeout_ms, Some(3000));
    assert_eq!(hc.retries, Some(1));
}
```

- [ ] **Step 2: Implement `resolve_health_check`**

```rust
/// Resolve the effective health check config for a profile.
/// Merges: profile.health_check → backend.health_check_url → supervisor defaults.
pub fn resolve_health_check(&self, profile: &ProfileConfig) -> HealthCheck {
    let backend = self.backends.get(&profile.backend);
    let profile_hc = profile.health_check.as_ref();

    HealthCheck {
        url: profile_hc
            .and_then(|h| h.url.clone())
            .or_else(|| backend.and_then(|b| b.health_check_url.clone())),
        interval_ms: Some(
            profile_hc
                .and_then(|h| h.interval_ms)
                .unwrap_or(self.supervisor.health_check_interval_ms),
        ),
        timeout_ms: Some(
            profile_hc
                .and_then(|h| h.timeout_ms)
                .unwrap_or(3000),
        ),
        retries: Some(
            profile_hc
                .and_then(|h| h.retries)
                .unwrap_or(1),
        ),
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p kronk-core -- config`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/kronk-core/src/config.rs
git commit -m "feat: add resolve_health_check for merging profile/backend/supervisor defaults"
```

### Task 3: Update ProcessSupervisor to use HealthCheck

**Files:**
- Modify: `crates/kronk-core/src/process.rs`
- Modify: `crates/kronk-cli/src/main.rs`

- [ ] **Step 1: Update ProcessSupervisor to accept HealthCheck**

Change the constructor to accept a `HealthCheck` struct (or its resolved fields) instead of separate `health_url` and `health_check_interval_ms` params. Update the HTTP client timeout to use `timeout_ms` and add retry loop logic for `retries`.

- [ ] **Step 2: Update all call sites in main.rs**

Use `config.resolve_health_check(profile)` to get the merged config and pass it to `ProcessSupervisor::new`.

- [ ] **Step 3: Verify it compiles and tests pass**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/kronk-core/src/process.rs crates/kronk-cli/src/main.rs
git commit -m "feat: ProcessSupervisor uses resolved HealthCheck config"
```
