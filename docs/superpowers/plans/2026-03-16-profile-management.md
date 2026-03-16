# Profile Management Subcommand Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate profile operations into a `kronk profile` subcommand group with `ls`, `add`, `edit`, and `rm` commands. Migrate the existing top-level `kronk add` and `kronk update` commands into this group (keeping them as hidden aliases for backward compatibility).

**Architecture:** Add a `ProfileCommands` enum in `main.rs` mirroring the pattern used by `ModelCommands` and `ServiceCommands`. The `add` and `edit` subcommands reuse the existing `cmd_add` logic. The new `ls` command displays all profiles with their backend, model, use case, and service status. The `rm` command removes a profile from config after checking for active services.

**Tech Stack:** Rust, clap (derive), existing `kronk-core` config system, `kronk-core::platform` for service status checks

---

## File Structure

### Files to modify

| File | Changes |
|------|---------|
| `crates/kronk-cli/src/main.rs` | Add `Profile` variant to `Commands` enum, add `ProfileCommands` enum, wire match arm, keep `Add`/`Update` as hidden aliases |
| `crates/kronk-core/src/config.rs` | Add `remove_profile` method to `Config` |

---

## Chunk 1: Profile Subcommand Group

### Task 1: Add `ProfileCommands` enum and wire into CLI

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`

- [ ] **Step 1: Add `ProfileCommands` enum**

Add after the existing `ModelCommands` enum:

```rust
#[derive(Parser, Debug)]
pub enum ProfileCommands {
    /// List all profiles with status
    Ls,
    /// Add a new profile from a raw command line
    Add {
        /// Profile name
        name: String,
        /// Backend command and arguments (e.g. llama-server -m model.gguf)
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Edit an existing profile's command line
    Edit {
        /// Profile name
        name: String,
        /// New backend command and arguments
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Remove a profile
    Rm {
        /// Profile name to remove
        name: String,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
}
```

- [ ] **Step 2: Add `Profile` variant to `Commands` enum**

```rust
/// Manage profiles — list, add, edit, remove
Profile {
    #[command(subcommand)]
    command: ProfileCommands,
},
```

- [ ] **Step 3: Mark existing `Add` and `Update` as hidden aliases**

Add `#[command(hide = true)]` to the existing `Add` and `Update` variants in the `Commands` enum so they still work but don't show in `--help`.

- [ ] **Step 4: Add the match arm in the async dispatch block**

```rust
Commands::Profile { command } => cmd_profile(&config, command).await,
```

- [ ] **Step 5: Verify it compiles (will fail — `cmd_profile` doesn't exist yet)**

Run: `cargo check -p kronk`
Expected: Error about missing `cmd_profile` function

### Task 2: Implement `cmd_profile` handler

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`

- [ ] **Step 1: Write `cmd_profile` function**

```rust
async fn cmd_profile(config: &Config, command: ProfileCommands) -> Result<()> {
    match command {
        ProfileCommands::Ls => cmd_profile_ls(config).await,
        ProfileCommands::Add { name, command } => cmd_add(config, &name, command, false),
        ProfileCommands::Edit { name, command } => cmd_add(config, &name, command, true),
        ProfileCommands::Rm { name, force } => cmd_profile_rm(config, &name, force),
    }
}
```

- [ ] **Step 2: Write `cmd_profile_ls`**

```rust
async fn cmd_profile_ls(config: &Config) -> Result<()> {
    if config.profiles.is_empty() {
        println!("No profiles configured.");
        println!();
        println!("Add one:  kronk profile add <name> <command...>");
        println!("Or pull:  kronk model pull <repo>");
        return Ok(());
    }

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    println!("Profiles:");
    println!("{}", "-".repeat(60));

    for (name, profile) in &config.profiles {
        let backend = config.backends.get(&profile.backend);
        let use_case = profile.use_case.as_ref()
            .map(|uc| uc.to_string())
            .unwrap_or_else(|| "none".to_string());

        let service_name = Config::service_name(name);
        let service_status = {
            #[cfg(target_os = "windows")]
            { kronk_core::platform::windows::query_service(&service_name).unwrap_or_else(|_| "UNKNOWN".to_string()) }
            #[cfg(target_os = "linux")]
            { kronk_core::platform::linux::query_service(&service_name).unwrap_or_else(|_| "UNKNOWN".to_string()) }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            { let _ = &service_name; "N/A".to_string() }
        };

        let health = if let Some(url) = backend.and_then(|b| b.health_check_url.as_ref()) {
            match http_client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => "HEALTHY",
                _ => "DOWN",
            }
        } else { "N/A" };

        println!();
        println!("  {}  (backend: {})", name, profile.backend);
        println!("    use-case: {}  service: {}  health: {}", use_case, service_status, health);

        if let Some(ref model) = profile.model {
            let quant = profile.quant.as_deref().unwrap_or("?");
            println!("    model: {} / {}", model, quant);
        }

        if !profile.args.is_empty() {
            let args_str = profile.args.join(" ");
            if args_str.len() > 80 {
                println!("    args: {}...", &args_str[..77]);
            } else {
                println!("    args: {}", args_str);
            }
        }
    }

    println!();
    Ok(())
}
```

- [ ] **Step 3: Write `cmd_profile_rm`**

```rust
fn cmd_profile_rm(config: &Config, name: &str, force: bool) -> Result<()> {
    if !config.profiles.contains_key(name) {
        anyhow::bail!("Profile '{}' not found.", name);
    }

    // Check if a service is installed for this profile
    let service_name = Config::service_name(name);
    let service_installed = {
        #[cfg(target_os = "windows")]
        { kronk_core::platform::windows::query_service(&service_name)
            .map(|s| s != "NOT_INSTALLED")
            .unwrap_or(false) }
        #[cfg(target_os = "linux")]
        { kronk_core::platform::linux::query_service(&service_name)
            .map(|s| s != "NOT_INSTALLED")
            .unwrap_or(false) }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        { let _ = &service_name; false }
    };

    if service_installed {
        anyhow::bail!(
            "Profile '{}' has an installed service '{}'. Remove it first with: kronk service remove --profile {}",
            name, service_name, name
        );
    }

    if !force {
        let confirm = inquire::Confirm::new(&format!("Remove profile '{}'?", name))
            .with_default(false)
            .prompt()
            .context("Confirmation cancelled")?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let mut config = config.clone();
    config.profiles.remove(name);
    config.save()?;

    println!("Profile '{}' removed.", name);
    Ok(())
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles with no errors

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/kronk-cli/src/main.rs
git commit -m "feat: add 'kronk profile' subcommand with ls, add, edit, rm"
```
