# Multi-Port Support Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow different profiles to run on different ports, with per-profile health check URLs and automatic firewall rules matching each profile's port.

**Architecture:** Add an optional `port` field to `ProfileConfig`. When set, `build_args` injects `--port <N>` into the argument list (if not already present) and the health check URL is auto-derived as `http://localhost:{port}/health` (unless explicitly overridden). The Windows firewall rule uses the profile's port instead of the hardcoded 8080. Service install and `cmd_status` use the resolved port for health checks.

**Tech Stack:** Rust, existing `kronk-core` config system, serde/toml, `kronk-core::platform::windows` firewall functions

---

## File Structure

### Files to modify

| File | Changes |
|------|---------|
| `crates/kronk-core/src/config.rs` | Add `port` field to `ProfileConfig`, add `resolve_health_url` method, update `build_args` to inject port, update `Config::default()` |
| `crates/kronk-cli/src/main.rs` | Update `cmd_status` and service install to use resolved port/health URL |
| `crates/kronk-core/src/platform/windows.rs` | Update `install_service` to accept port parameter for firewall rule |

---

## Chunk 1: Port Field and Argument Injection

### Task 1: Add `port` field to `ProfileConfig`

**Files:**
- Modify: `crates/kronk-core/src/config.rs`

- [ ] **Step 1: Write failing test**

Add to config.rs tests:

```rust
#[test]
fn test_profile_port_roundtrip() {
    let profile = ProfileConfig {
        backend: "llama_cpp".to_string(),
        args: vec![],
        use_case: None,
        sampling: None,
        model: None,
        quant: None,
        port: Some(8081),
    };
    let toml_str = toml::to_string_pretty(&profile).unwrap();
    let loaded: ProfileConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(loaded.port, Some(8081));
}

#[test]
fn test_profile_without_port_defaults_to_none() {
    let toml_str = r#"
backend = "llama_cpp"
args = []
"#;
    let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(profile.port, None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kronk-core -- test_profile_port`
Expected: FAIL — `port` field doesn't exist

- [ ] **Step 3: Add `port` field to `ProfileConfig`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub backend: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub use_case: Option<UseCase>,
    #[serde(default)]
    pub sampling: Option<SamplingParams>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    /// Port this profile's backend listens on. If set, injects `--port` into args.
    #[serde(default)]
    pub port: Option<u16>,
}
```

Update `Config::default()` to include `port: None` in the default profile.

- [ ] **Step 4: Fix all existing `ProfileConfig` literals in tests**

Add `port: None` to every test that constructs a `ProfileConfig` directly.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p kronk-core -- config`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/kronk-core/src/config.rs
git commit -m "feat: add optional port field to ProfileConfig"
```

### Task 2: Add `resolve_health_url` and update `build_args`

**Files:**
- Modify: `crates/kronk-core/src/config.rs`

- [ ] **Step 1: Write failing test for port injection in build_args**

```rust
#[test]
fn test_build_args_injects_port() {
    let config = Config::default();
    let mut profile = config.profiles.get("default").unwrap().clone();
    profile.port = Some(9090);
    let backend = config.backends.get(&profile.backend).unwrap();
    let args = config.build_args(&profile, backend);
    assert!(args.contains(&"--port".to_string()));
    assert!(args.contains(&"9090".to_string()));
}

#[test]
fn test_build_args_no_port_when_none() {
    let config = Config::default();
    let profile = config.profiles.get("default").unwrap();
    let backend = config.backends.get(&profile.backend).unwrap();
    let args = config.build_args(profile, backend);
    assert!(!args.contains(&"--port".to_string()));
}
```

- [ ] **Step 2: Update `build_args` to inject port**

In the `build_args` method, after extending with profile args but before sampling, add:

```rust
// Inject --port if configured and not already present
if let Some(port) = profile.port {
    if !args.iter().any(|a| a == "--port" || a == "-p") {
        args.push("--port".to_string());
        args.push(port.to_string());
    }
}
```

- [ ] **Step 3: Add `resolve_health_url` method**

```rust
/// Resolve the health check URL for a profile.
/// Uses the backend's health_check_url if set, otherwise derives from the profile's port.
pub fn resolve_health_url(&self, profile: &ProfileConfig) -> Option<String> {
    let backend = self.backends.get(&profile.backend)?;
    if let Some(ref url) = backend.health_check_url {
        // If profile has a custom port, replace the port in the URL
        if let Some(port) = profile.port {
            if let Ok(mut parsed) = url::Url::parse(url) {
                let _ = parsed.set_port(Some(port));
                return Some(parsed.to_string());
            }
        }
        Some(url.clone())
    } else if let Some(port) = profile.port {
        Some(format!("http://localhost:{}/health", port))
    } else {
        None
    }
}
```

Note: This requires the `url` crate. Add `url = "2"` to workspace dependencies, or use simple string replacement instead to avoid the dep.

- [ ] **Step 4: Run tests**

Run: `cargo test -p kronk-core -- config`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/kronk-core/src/config.rs
git commit -m "feat: build_args injects --port, add resolve_health_url"
```

### Task 3: Update CLI and firewall to use per-profile ports

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`
- Modify: `crates/kronk-core/src/platform/windows.rs`

- [ ] **Step 1: Update `cmd_status` to use `resolve_health_url`**

Replace direct `backend.health_check_url` lookups in `cmd_status` with `config.resolve_health_url(profile)`.

- [ ] **Step 2: Update Windows firewall in service install**

Change `add_firewall_rule(service_name, 8080)` to use the profile's port:

```rust
let port = profile.port.unwrap_or(8080);
add_firewall_rule(service_name, port).ok();
```

- [ ] **Step 3: Update `cmd_profile_ls` and `cmd_ps` (in model.rs) to use `resolve_health_url`**

Replace direct health check URL lookups with `config.resolve_health_url(profile)`.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles with no errors

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: use per-profile port for health checks and firewall rules"
```
