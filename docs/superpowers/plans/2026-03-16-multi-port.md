# Multi-Port Support Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow different profiles to run on different ports, with per-profile health check URLs and automatic firewall rules matching each profile's port.

**Architecture:** Add an optional `port` field to `ProfileConfig`. When set, `build_args` injects `--port <N>` into the argument list (if not already present) and the health check URL is auto-derived as `http://localhost:{port}/health` (unless explicitly overridden). The Windows firewall rule uses the profile's port instead of the hardcoded 8080. Service install and `cmd_status` use the resolved port for health checks.

**Tech Stack:** Rust, existing `kronk-core` config system, serde/toml, `url` crate, `kronk-core::platform::windows` firewall functions

---

## File Structure

### Files to modify

| File | Changes |
|------|---------|
| `Cargo.toml` | Add `url` to workspace dependencies |
| `crates/kronk-core/Cargo.toml` | Add `url` to dependencies |
| `crates/kronk-core/src/config.rs` | Add `port` field to `ProfileConfig`, add `resolve_health_url` method, update `build_args` to inject port, update `Config::default()` |
| `crates/kronk-cli/src/main.rs` | Update `cmd_status`, `cmd_profile_ls` and service install to use resolved port/health URL, pass port to `install_service` |
| `crates/kronk-cli/src/commands/model.rs` | Update `cmd_ps` to use resolved port/health URL |
| `crates/kronk-core/src/platform/windows.rs` | Update `install_service` to accept port parameter for firewall rule |

---

## Chunk 1: Port Field and Argument Injection

### Task 1: Add `url` crate dependency

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/kronk-core/Cargo.toml`

- [ ] **Step 1: Add to workspace dependencies**

Modify `Cargo.toml` `[workspace.dependencies]` block to include `url`:
```toml
url = "2"
```

- [ ] **Step 2: Add to kronk-core dependencies**

Modify `crates/kronk-core/Cargo.toml` `[dependencies]` block to use the workspace dependency:
```toml
url = { workspace = true }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p kronk-core`
Expected: Compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/kronk-core/Cargo.toml
git commit -m "build: add url crate dependency"
```

### Task 2: Add `port` field to `ProfileConfig`

**Files:**
- Modify: `crates/kronk-core/src/config.rs`

- [ ] **Step 1: Write failing test**

Add to `config.rs` tests:

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
    /// Model card reference in "company/modelname" format.
    #[serde(default)]
    pub model: Option<String>,
    /// Which quant to use from the model card (e.g. "Q4_K_M").
    #[serde(default)]
    pub quant: Option<String>,
    /// Port this profile's backend listens on. If set, injects `--port` into args.
    #[serde(default)]
    pub port: Option<u16>,
}
```

Update `Config::default()` to include `port: None` in the default profile.

- [ ] **Step 4: Fix all existing `ProfileConfig` literals in tests**

Add `port: None` to every test that constructs a `ProfileConfig` directly in `config.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p kronk-core -- config`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/kronk-core/src/config.rs
git commit -m "feat: add optional port field to ProfileConfig"
```

### Task 3: Add `resolve_health_url` and update `build_args`

**Files:**
- Modify: `crates/kronk-core/src/config.rs`

- [ ] **Step 1: Write failing tests**

Add these tests to `config.rs`:

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

#[test]
fn test_build_args_does_not_duplicate_port() {
    let config = Config::default();
    let mut profile = config.profiles.get("default").unwrap().clone();
    profile.port = Some(9090);
    profile.args.push("--port".to_string());
    profile.args.push("8080".to_string());
    let backend = config.backends.get(&profile.backend).unwrap();
    let args = config.build_args(&profile, backend);
    
    // Should keep the user's --port 8080 and not inject 9090
    assert!(args.contains(&"8080".to_string()));
    assert!(!args.contains(&"9090".to_string()));
}

#[test]
fn test_resolve_health_url_overrides_port() {
    let config = Config::default();
    let mut profile = config.profiles.get("default").unwrap().clone();
    profile.port = Some(9090);
    
    let url = config.resolve_health_url(&profile).unwrap();
    assert_eq!(url, "http://localhost:9090/health");
}

#[test]
fn test_resolve_health_url_fallback() {
    let mut config = Config::default();
    // Remove default health_check_url
    if let Some(backend) = config.backends.get_mut("llama_cpp") {
        backend.health_check_url = None;
    }
    
    let mut profile = config.profiles.get("default").unwrap().clone();
    profile.port = Some(9091);
    
    // Should fallback to localhost with port
    let url = config.resolve_health_url(&profile).unwrap();
    assert_eq!(url, "http://localhost:9091/health");
}
```

- [ ] **Step 2: Update `build_args` to inject port**

In the `build_args` method of `Config`, after extending with profile args but before sampling, add:

```rust
        // Inject --port if configured and not already present
        if let Some(port) = profile.port {
            if !args.iter().any(|a| a == "--port" || a == "-p") {
                args.push("--port".to_string());
                args.push(port.to_string());
            }
        }
```

- [ ] **Step 3: Add `resolve_health_url` method to `Config`**

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

- [ ] **Step 4: Run tests**

Run: `cargo test -p kronk-core -- config`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/kronk-core/src/config.rs
git commit -m "feat: build_args injects --port, add resolve_health_url"
```

### Task 4: Update CLI and firewall to use per-profile ports

**Files:**
- Modify: `crates/kronk-core/src/platform/windows.rs`
- Modify: `crates/kronk-cli/src/main.rs`
- Modify: `crates/kronk-cli/src/commands/model.rs`

- [ ] **Step 1: Update Windows firewall in service install**

In `crates/kronk-core/src/platform/windows.rs`, update `install_service` signature to accept a `port` parameter and use it for the firewall rule:

```rust
pub fn install_service(
    service_name: &str,
    display_name: &str,
    profile: &str,
    config_dir: &std::path::Path,
    port: u16,
) -> Result<()> {
// ...
    // Add firewall rule for the server port
    add_firewall_rule(service_name, port).ok();
// ...
```

- [ ] **Step 2: Update `cmd_service` Install in `main.rs`**

In `crates/kronk-cli/src/main.rs` within `cmd_service` `ServiceCommands::Install` branch, pass the resolved port:

```rust
            #[cfg(target_os = "windows")]
            {
                let display_name = format!("Kronk: {}", profile);
                let config_dir = Config::base_dir()?;
                let port = prof.port.unwrap_or(8080);
                kronk_core::platform::windows::install_service(
                    &service_name,
                    &display_name,
                    &profile,
                    &config_dir,
                    port,
                )?;
            }
```

- [ ] **Step 3: Update `cmd_status` and `cmd_profile_ls` in `main.rs`**

In `cmd_status` and `cmd_profile_ls`, replace `backend.and_then(|b| b.health_check_url.as_ref())` check with `config.resolve_health_url(profile)`.
Note that `config.resolve_health_url` takes a `&ProfileConfig`, so pass `profile` and `.as_ref()` for string mapping:

```rust
        // Check health endpoint
        let health = if let Some(url) = config.resolve_health_url(profile) {
            match http_client.get(&url).send().await {
```

- [ ] **Step 4: Update `cmd_ps` in `model.rs` to use `resolve_health_url`**

In `crates/kronk-cli/src/commands/model.rs` `cmd_ps` function:
```rust
        let health = if let Some(url) = config.resolve_health_url(profile) {
            match http_client.get(&url).send().await {
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles with no errors

- [ ] **Step 6: Run all tests**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: use per-profile port for health checks and firewall rules"
```