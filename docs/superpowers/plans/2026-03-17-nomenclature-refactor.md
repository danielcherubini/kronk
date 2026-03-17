# Nomenclature Refactor: Profile → Server, UseCase → Profile

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename `profile` → `server`, `use_case` → `profile` across the entire codebase, restructure config directories to use `profiles.d/` for sampling presets and `configs.d/` for model cards, add an `enabled` flag to servers, and make `kronk run` require a positional server name. This is groundwork for future multi-server support.

**Architecture:** Three-phase refactor: (1) rename all types, fields, CLI commands and config keys, (2) restructure directories — move hardcoded sampling presets into `profiles.d/*.toml` and model cards into `configs.d/<company>--<model>.toml`, (3) add `enabled` flag to servers and update service commands to operate on all enabled servers when no name argument is given.

**Tech Stack:** Rust, clap (derive), serde, toml, tokio

---

## File Structure

### Files Modified

| File | Responsibility |
|------|---------------|
| `crates/kronk-core/src/lib.rs` | Module declarations — rename `use_cases` → `profiles` |
| `crates/kronk-core/src/config.rs` | `Config`, `ServerConfig` (was `ProfileConfig`), merge logic, directory resolution |
| `crates/kronk-core/src/profiles.rs` | (was `use_cases.rs`) `SamplingParams`, `Profile` enum (was `UseCase`), TOML loading from `profiles.d/` |
| `crates/kronk-core/src/models/card.rs` | `ModelCard` — update doc comments referencing `use_case` → `profile` |
| `crates/kronk-core/src/models/registry.rs` | `ModelRegistry` — update to scan `configs.d/` instead of `models/<company>/<model>/model.toml` |
| `crates/kronk-core/src/platform/linux.rs` | systemd unit generation — update comments/descriptions |
| `crates/kronk-cli/src/main.rs` | CLI definitions, all command handlers |
| `crates/kronk-cli/src/commands/model.rs` | Model commands — update profile→server references |
| `crates/kronk-cli/src/args.rs` | No changes needed |
| `crates/kronk-cli/src/tests.rs` | No changes needed (tests `inject_context_size` only) |
| `modelcards/Tesslate/OmniCoder-9B.toml` | Community card — no changes needed (`[sampling.coding]` key names stay the same) |
| `README.md` | Update all documentation |

### Files Created

| File | Responsibility |
|------|---------------|
| `crates/kronk-core/src/profiles.rs` | Renamed from `use_cases.rs` — adds `load_profiles_d()` to load TOML presets from `profiles.d/` |

### Files Deleted

| File | Reason |
|------|--------|
| `crates/kronk-core/src/use_cases.rs` | Renamed to `profiles.rs` |

---

## Terminology Mapping (Reference)

| Old Term | New Term | Struct/Enum | Config TOML Key |
|----------|----------|-------------|-----------------|
| `ProfileConfig` | `ServerConfig` | `ServerConfig` | `[servers.<name>]` |
| `UseCase` | `Profile` | `Profile` | `profile = "coding"` |
| `use_case` field | `profile` field | `ServerConfig.profile` | `profile = "coding"` |
| `custom_use_cases` | `custom_profiles` | `Config.custom_profiles` | `[custom_profiles.<name>]` |
| `profiles` (HashMap) | `servers` (HashMap) | `Config.servers` | `[servers.<name>]` |
| `SamplingParams` | `SamplingParams` | (unchanged) | (unchanged) |

---

## Task 1: Rename `use_cases.rs` → `profiles.rs` and `UseCase` → `Profile`

This task renames the enum and module. No behavioral changes.

**Files:**
- Rename: `crates/kronk-core/src/use_cases.rs` → `crates/kronk-core/src/profiles.rs`
- Modify: `crates/kronk-core/src/lib.rs`
- Modify: `crates/kronk-core/src/config.rs` (imports + field names)
- Modify: `crates/kronk-cli/src/main.rs` (imports)
- Modify: `crates/kronk-cli/src/commands/model.rs` (imports)

- [ ] **Step 1: Rename the file**

```bash
mv crates/kronk-core/src/use_cases.rs crates/kronk-core/src/profiles.rs
```

- [ ] **Step 2: Update `lib.rs` module declaration**

In `crates/kronk-core/src/lib.rs`, change:
```rust
pub mod use_cases;
```
to:
```rust
pub mod profiles;
```

- [ ] **Step 3: Rename `UseCase` → `Profile` in `profiles.rs`**

In `crates/kronk-core/src/profiles.rs`:
- Rename enum `UseCase` → `Profile` (all occurrences)
- Rename `UseCase::Coding` → `Profile::Coding`, etc.
- Update `UseCase::all()` → `Profile::all()`
- Update `UseCase::params()` → `Profile::params()`
- Update `impl Display for UseCase` → `impl Display for Profile`
- Update `impl FromStr for UseCase` → `impl FromStr for Profile`
- Update all doc comments referencing "use case" → "profile"
- Update all test references

- [ ] **Step 4: Update imports in `config.rs`**

In `crates/kronk-core/src/config.rs`:
```rust
// Change:
use crate::use_cases::{SamplingParams, UseCase};
// To:
use crate::profiles::{SamplingParams, Profile};
```

Then rename all occurrences of `UseCase` → `Profile` in config.rs:
- All pattern matches: `UseCase::Custom { name }` → `Profile::Custom { name }`
- All method calls: `UseCase::Coding` → `Profile::Coding`

- [ ] **Step 5: Update imports in `main.rs`**

In `crates/kronk-cli/src/main.rs`:
```rust
// Change:
use kronk_core::use_cases::SamplingParams;
// To:
use kronk_core::profiles::SamplingParams;
```

Also in `cmd_use_case()`:
```rust
// Change:
use kronk_core::use_cases::UseCase;
// To:
use kronk_core::profiles::Profile;
```

And rename all `UseCase::` references to `Profile::` in the function.

- [ ] **Step 6: Update imports in `commands/model.rs`**

In `crates/kronk-cli/src/commands/model.rs`:
```rust
// Change:
let resolved_use_case: Option<kronk_core::use_cases::UseCase> = ...
// To:
let resolved_profile: Option<kronk_core::profiles::Profile> = ...
```

- [ ] **Step 7: Build and test**

```bash
cargo build -p kronk-core && cargo build -p kronk && cargo test --workspace
```
Expected: compiles, all 54+ tests pass.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: rename UseCase → Profile, use_cases.rs → profiles.rs"
```

---

## Task 2: Rename `ProfileConfig` → `ServerConfig`, `profiles` → `servers` in config

This task renames the config struct and HashMap key. The TOML config key changes from `[profiles.x]` to `[servers.x]` and `use_case` field to `profile`.

**Files:**
- Modify: `crates/kronk-core/src/config.rs`
- Modify: `crates/kronk-cli/src/main.rs`
- Modify: `crates/kronk-cli/src/commands/model.rs`

- [ ] **Step 1: Rename struct and field in `config.rs`**

In `crates/kronk-core/src/config.rs`:

Rename `ProfileConfig` → `ServerConfig`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub backend: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub profile: Option<Profile>,       // was use_case
    #[serde(default)]
    pub sampling: Option<SamplingParams>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub health_check: Option<HealthCheck>,
    /// Whether this server is active for multi-server operations.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}
```

Rename `Config.profiles` → `Config.servers`:
```rust
pub struct Config {
    pub general: General,
    pub backends: HashMap<String, BackendConfig>,
    pub servers: HashMap<String, ServerConfig>,  // was profiles
    pub supervisor: Supervisor,
    #[serde(default)]
    pub custom_profiles: Option<HashMap<String, SamplingParams>>,  // was custom_use_cases
    #[serde(skip)]
    pub loaded_from: Option<PathBuf>,
}
```

Rename `resolve_profile` → `resolve_server`:
```rust
pub fn resolve_server(&self, name: &str) -> Result<(&ServerConfig, &BackendConfig)> {
    let server = self.servers.get(name)
        .with_context(|| format!("Server '{}' not found in config", name))?;
    let backend = self.backends.get(&server.backend).with_context(|| {
        format!("Backend '{}' referenced by server '{}' not found in config",
            server.backend, name)
    })?;
    Ok((server, backend))
}
```

Update `build_args` signature: parameter names from `profile` → `server`.

Update `effective_sampling` / `effective_sampling_with_card`: change `profile.use_case` → `server.profile`, parameter names from `profile` → `server`.

Update `Config::default()` to use `servers` instead of `profiles`:
```rust
let mut servers = HashMap::new();
servers.insert(
    "default".to_string(),
    ServerConfig {
        backend: "llama_cpp".to_string(),
        args: vec![...],
        profile: Some(Profile::Coding),  // was use_case
        sampling: None,
        model: None,
        quant: None,
        port: None,
        health_check: None,
        enabled: true,
    },
);
```

Update all tests in `config.rs` to use new names.

- [ ] **Step 2: Update all references in `main.rs`**

Bulk rename in `crates/kronk-cli/src/main.rs`:
- All `config.profiles` → `config.servers`
- All `config.resolve_profile` → `config.resolve_server`
- All `ProfileConfig` → `ServerConfig`
- All `profile.use_case` → `server.profile` (in the struct field context)
- All `custom_use_cases` → `custom_profiles`
- Function parameter names: `profile: &ProfileConfig` → `server: &ServerConfig` where appropriate
- User-facing strings: "Profile" → "Server" in println messages
- Error messages: "Profile '{}' not found" → "Server '{}' not found"

- [ ] **Step 3: Update all references in `commands/model.rs`**

Similar bulk rename:
- `config.profiles` → `config.servers`
- `ProfileConfig` → `ServerConfig`
- `profile.model` → `server.model`
- User-facing strings: "Profile" → "Server", "profile" → "server"

- [ ] **Step 4: Build and test**

```bash
cargo build --workspace && cargo test --workspace
```
Expected: compiles, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: rename ProfileConfig → ServerConfig, profiles → servers in config"
```

---

## Task 3: Rename CLI commands `profile` → `server`, `use-case` → `profile`

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`

- [ ] **Step 1: Rename CLI command enums**

In `crates/kronk-cli/src/main.rs`:

Rename `ProfileCommands` → `ServerCommands`:
```rust
#[derive(Parser, Debug)]
pub enum ServerCommands {
    /// List all servers with status
    Ls,
    /// Add a new server from a raw command line
    Add { name: String, #[arg(...)] command: Vec<String> },
    /// Edit an existing server's command line
    Edit { name: String, #[arg(...)] command: Vec<String> },
    /// Remove a server
    Rm { name: String, #[arg(long)] force: bool },
}
```

Rename `UseCaseCommands` → `ProfileCommands`:
```rust
#[derive(Parser, Debug)]
enum ProfileCommands {
    /// List all available profiles and their sampling params
    List,
    /// Set a server's profile
    Set { server: String, profile: String },
    /// Clear a server's profile (remove sampling preset)
    Clear { server: String },
    /// Create a custom profile with specific sampling params
    Add { name: String, ... },
    /// Remove a custom profile
    Remove { name: String },
}
```

Update `Commands` enum:
```rust
enum Commands {
    /// Run a server in the foreground (for testing)
    Run {
        /// Server name (required)
        name: String,
        #[arg(long)]
        ctx: Option<u32>,
    },
    /// Manage services
    Service { #[command(subcommand)] command: ServiceCommands },
    #[command(hide = true)]
    ServiceRun { ... },
    /// Add a new server from a raw command line
    #[command(hide = true)]
    Add { name: String, command: Vec<String> },
    /// Update an existing server
    #[command(hide = true)]
    Update { name: String, command: Vec<String> },
    /// Manage servers — list, add, edit, remove
    Server { #[command(subcommand)] command: ServerCommands },
    /// Show status of all servers
    Status,
    /// Manage sampling profiles — presets for inference params
    Profile { #[command(subcommand)] command: ProfileCommands },
    /// View or edit configuration
    Config { #[command(subcommand)] command: ConfigCommands },
    /// Manage models — pull, list, create servers
    Model { #[command(subcommand)] command: ModelCommands },
    /// View backend logs for a server
    Logs {
        /// Server name (required)
        name: String,
        #[arg(short, long)]
        follow: bool,
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
    },
}
```

Note: `Run` changes from `--profile` flag to positional `name` (required, no default).

- [ ] **Step 2: Rename ServiceCommands fields**

Change all `profile` fields to `name` in `ServiceCommands`:
```rust
#[derive(Parser, Debug)]
enum ServiceCommands {
    Install {
        /// Server name (omit to install all enabled servers)
        name: Option<String>,
    },
    Start {
        /// Server name (omit to start all enabled servers)
        name: Option<String>,
    },
    Stop {
        /// Server name (omit to stop all enabled servers)
        name: Option<String>,
    },
    Remove {
        /// Server name (omit to remove all enabled servers)
        name: Option<String>,
    },
}
```

- [ ] **Step 3: Rename handler functions**

- `cmd_profile()` → `cmd_server()`
- `cmd_profile_ls()` → `cmd_server_ls()`
- `cmd_profile_rm()` → `cmd_server_rm()`
- `cmd_profile_add()` → `cmd_server_add()`
- `cmd_profile_edit()` → `cmd_server_edit()`
- `cmd_use_case()` → `cmd_profile()`

Update all user-facing strings in these functions.

- [ ] **Step 4: Update `cmd_run` signature**

```rust
async fn cmd_run(config: &Config, server_name: &str, ctx_override: Option<u32>) -> Result<()> {
    let (server, backend) = config.resolve_server(server_name)?;
    // ... update all internal references
}
```

- [ ] **Step 5: Update `cmd_service` for optional name**

```rust
fn cmd_service(config: &Config, command: ServiceCommands) -> Result<()> {
    match command {
        ServiceCommands::Install { name } => {
            let names = resolve_server_names(config, name)?;
            for name in &names {
                // ... install each
            }
        }
        // similar for Start, Stop, Remove
    }
}

/// Resolve server names: if given, use that one; if None, use all enabled.
fn resolve_server_names(config: &Config, name: Option<String>) -> Result<Vec<String>> {
    match name {
        Some(n) => {
            config.resolve_server(&n)?; // validate it exists
            Ok(vec![n])
        }
        None => {
            let enabled: Vec<String> = config.servers.iter()
                .filter(|(_, s)| s.enabled)
                .map(|(n, _)| n.clone())
                .collect();
            if enabled.is_empty() {
                anyhow::bail!("No enabled servers. Enable one with `kronk config edit`.");
            }
            Ok(enabled)
        }
    }
}
```

- [ ] **Step 6: Update match arms in `main()`**

```rust
match args.command {
    Commands::Run { name, ctx } => cmd_run(&config, &name, ctx).await,
    Commands::Service { command } => cmd_service(&config, command),
    Commands::ServiceRun { .. } => { /* ... */ },
    Commands::Add { name, command } => cmd_server_add(&config, &name, command, false).await,
    Commands::Update { name, command } => cmd_server_edit(&mut config.clone(), &name, command).await,
    Commands::Server { command } => cmd_server(&config, command).await,
    Commands::Status => cmd_status(&config).await,
    Commands::Profile { command } => cmd_profile(&config, command),
    Commands::Config { command } => cmd_config(&config, command),
    Commands::Model { command } => commands::model::run(&config, command).await,
    Commands::Logs { name, follow, lines } => cmd_logs(&config, &name, follow, lines).await,
}
```

- [ ] **Step 7: Update `model.rs` CLI output strings and `ModelCommands::Create`**

In `crates/kronk-cli/src/commands/model.rs`:
- Change "Create a profile:" → "Create a server:"
- Change `kronk model create my-profile` → `kronk model create my-server`
- Change `kronk run --profile` → `kronk run <name>`
- Change `kronk service install --profile` → `kronk service install <name>`
- Rename `use_case` field to `profile` in `ModelCommands::Create`:

```rust
Create {
    name: String,
    #[arg(long)]
    model: String,
    #[arg(long)]
    quant: Option<String>,
    /// Sampling profile: coding, chat, analysis, creative
    #[arg(long)]
    profile: Option<String>,
    #[arg(long)]
    backend: Option<String>,
},
```

- [ ] **Step 8: Build and test**

```bash
cargo build --workspace && cargo test --workspace
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor: rename CLI commands profile → server, use-case → profile"
```

---

## Task 4: Move model cards to `configs.d/`

Model cards currently live at `~/.config/kronk/models/<company>/<model>/model.toml`.
Move the canonical location to `~/.config/kronk/configs.d/<company>--<model>.toml`.
GGUF files remain in `models/<company>/<model>/`.

**Files:**
- Modify: `crates/kronk-core/src/config.rs` — add `configs_dir()` method
- Modify: `crates/kronk-core/src/models/registry.rs` — scan `configs.d/` instead of `models/**/model.toml`
- Modify: `crates/kronk-core/src/models/card.rs` — update doc comments
- Modify: `crates/kronk-cli/src/commands/model.rs` — update `cmd_pull` to save cards to `configs.d/`

- [ ] **Step 1: Add `configs_dir()` to `Config`**

In `crates/kronk-core/src/config.rs`:
```rust
/// Resolve the configs.d directory for model cards.
/// `<base_dir>/configs.d/`
pub fn configs_dir(&self) -> Result<PathBuf> {
    if let Some(ref loaded) = self.loaded_from {
        Ok(loaded.join("configs.d"))
    } else {
        Ok(Self::base_dir()?.join("configs.d"))
    }
}
```

- [ ] **Step 2: Update `ModelRegistry` to scan `configs.d/`**

In `crates/kronk-core/src/models/registry.rs`:

Change `ModelRegistry` to accept both `models_dir` and `configs_dir`:
```rust
pub struct ModelRegistry {
    models_dir: PathBuf,
    configs_dir: PathBuf,
}

impl ModelRegistry {
    pub fn new(models_dir: PathBuf, configs_dir: PathBuf) -> Self {
        Self { models_dir, configs_dir }
    }
}
```

Update `scan()` to read from `configs.d/`:
```rust
pub fn scan(&self) -> Result<Vec<InstalledModel>> {
    let mut models = Vec::new();
    if !self.configs_dir.exists() {
        return Ok(models);
    }
    for entry in std::fs::read_dir(&self.configs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.extension().map_or(false, |e| e == "toml") {
            continue;
        }
        // Filename: "company--modelname.toml" → id: "company/modelname"
        let stem = path.file_stem().unwrap().to_string_lossy();
        let id = match stem.find("--") {
            Some(pos) => format!("{}/{}", &stem[..pos], &stem[pos + 2..]),
            None => continue,
        };
        let model_dir = self.models_dir.join(&id);

        match ModelCard::load(&path) {
            Ok(card) => {
                models.push(InstalledModel {
                    dir: model_dir,
                    card,
                    id,
                    card_path: path,
                });
            }
            Err(e) => {
                tracing::warn!("Skipping malformed model card at {}: {}", path.display(), e);
            }
        }
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}
```

Add `card_path` to `InstalledModel`:
```rust
pub struct InstalledModel {
    pub dir: PathBuf,
    pub card: ModelCard,
    pub id: String,
    pub card_path: PathBuf,
}
```

- [ ] **Step 3: Update all `ModelRegistry::new()` call sites**

All call sites need the second argument:
```rust
let models_dir = config.models_dir()?;
let configs_dir = config.configs_dir()?;
let registry = ModelRegistry::new(models_dir, configs_dir);
```

Call sites:
- `crates/kronk-cli/src/main.rs` (in `build_full_args`)
- `crates/kronk-cli/src/commands/model.rs` (multiple places)

- [ ] **Step 4: Update `cmd_pull` to save cards to `configs.d/`**

In `crates/kronk-cli/src/commands/model.rs`, change card save path from:
```rust
let card_path = model_dir.join("model.toml");
```
to:
```rust
let configs_dir = config.configs_dir()?;
std::fs::create_dir_all(&configs_dir)?;
let card_filename = format!("{}.toml", model_id.replace('/', "--"));
let card_path = configs_dir.join(&card_filename);
```

- [ ] **Step 5: Update `cmd_scan` to use new card locations**

Update scan logic to save cards to `configs.d/` instead of `models/**/model.toml`.

- [ ] **Step 6: Add migration for existing model cards**

In `Config::load_from()`, after loading config, check if `models/` contains any `model.toml` files and no `configs.d/` exists. If so, migrate:

```rust
fn migrate_model_cards_to_configs_d(config: &Config) -> Result<()> {
    let configs_dir = config.configs_dir()?;
    if configs_dir.exists() {
        return Ok(()); // already migrated
    }
    let models_dir = config.models_dir()?;
    if !models_dir.exists() {
        return Ok(());
    }
    let mut migrated = false;
    for company_entry in std::fs::read_dir(&models_dir)? {
        let company_entry = company_entry?;
        if !company_entry.path().is_dir() { continue; }
        let company = company_entry.file_name().to_string_lossy().to_string();
        for model_entry in std::fs::read_dir(company_entry.path())? {
            let model_entry = model_entry?;
            let old_card = model_entry.path().join("model.toml");
            if old_card.exists() {
                let model_name = model_entry.file_name().to_string_lossy().to_string();
                let new_filename = format!("{}--{}.toml", company, model_name);
                std::fs::create_dir_all(&configs_dir)?;
                std::fs::copy(&old_card, configs_dir.join(&new_filename))?;
                std::fs::remove_file(&old_card)?;
                migrated = true;
            }
        }
    }
    if migrated {
        tracing::info!("Migrated model cards to {}", configs_dir.display());
    }
    Ok(())
}
```

- [ ] **Step 7: Update tests in `registry.rs`**

Update test helpers to create cards in `configs.d/` instead of `models/<company>/<model>/model.toml`:
```rust
fn setup_test_dir() -> (tempfile::TempDir, ModelRegistry) {
    let tmp = tempfile::tempdir().unwrap();
    let models = tmp.path().join("models");
    let configs = tmp.path().join("configs.d");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::create_dir_all(&configs).unwrap();
    let registry = ModelRegistry::new(models, configs);
    (tmp, registry)
}

fn create_test_model(base: &Path, company: &str, model: &str) -> ModelCard {
    let model_dir = base.join("models").join(company).join(model);
    let configs_dir = base.join("configs.d");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::create_dir_all(&configs_dir).unwrap();

    let card = ModelCard { /* ... */ };
    let card_filename = format!("{}--{}.toml", company, model);
    card.save(&configs_dir.join(&card_filename)).unwrap();
    // GGUF file still goes in models dir
    std::fs::write(model_dir.join(format!("{}-Q4_K_M.gguf", model)), b"fake").unwrap();
    card
}
```

- [ ] **Step 8: Build and test**

```bash
cargo build --workspace && cargo test --workspace
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor: move model cards to configs.d/<company>--<model>.toml"
```

---

## Task 5: Ship default profiles in `profiles.d/`

Move hardcoded sampling presets from the `Profile` enum into TOML files in `profiles.d/`. The built-in defaults become fallbacks only when `profiles.d/` is empty or missing a file.

**Files:**
- Modify: `crates/kronk-core/src/profiles.rs` — add `load_profiles_d()`, `generate_default_profiles()`
- Modify: `crates/kronk-core/src/config.rs` — add `profiles_dir()`, call generation on first run, use disk profiles in sampling merge

- [ ] **Step 1: Add `profiles_dir()` to `Config`**

In `crates/kronk-core/src/config.rs`:
```rust
pub fn profiles_dir(&self) -> Result<PathBuf> {
    if let Some(ref loaded) = self.loaded_from {
        Ok(loaded.join("profiles.d"))
    } else {
        Ok(Self::base_dir()?.join("profiles.d"))
    }
}
```

- [ ] **Step 2: Define TOML format for profile files**

Each file in `profiles.d/` is named after the profile:

`profiles.d/coding.toml`:
```toml
# Sampling profile for code generation and agentic tasks.
# Low temperature for deterministic, focused output.

[sampling]
temperature = 0.3
top_p = 0.9
top_k = 50
min_p = 0.05
presence_penalty = 0.1
```

`profiles.d/chat.toml`:
```toml
# Sampling profile for conversational use.
# Balanced temperature for natural responses.

[sampling]
temperature = 0.7
top_p = 0.95
top_k = 40
min_p = 0.05
presence_penalty = 0.0
```

`profiles.d/analysis.toml`:
```toml
# Sampling profile for data analysis and reasoning.
# Low temperature with focused sampling.

[sampling]
temperature = 0.3
top_p = 0.9
top_k = 20
min_p = 0.05
presence_penalty = 0.0
```

`profiles.d/creative.toml`:
```toml
# Sampling profile for creative writing and brainstorming.
# High temperature for diverse, exploratory output.

[sampling]
temperature = 0.9
top_p = 0.95
top_k = 50
min_p = 0.02
presence_penalty = 0.0
```

- [ ] **Step 3: Add `load_profiles_d()` and `generate_default_profiles()`**

In `crates/kronk-core/src/profiles.rs`:

```rust
use std::path::Path;
use std::collections::HashMap;

/// A profile definition loaded from profiles.d/<name>.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileDef {
    pub sampling: SamplingParams,
}

/// Load all profile definitions from the profiles.d directory.
pub fn load_profiles_d(dir: &Path) -> anyhow::Result<HashMap<String, SamplingParams>> {
    let mut profiles = HashMap::new();
    if !dir.exists() {
        return Ok(profiles);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "toml") {
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let contents = std::fs::read_to_string(&path)?;
            match toml::from_str::<ProfileDef>(&contents) {
                Ok(def) => { profiles.insert(name, def.sampling); }
                Err(e) => { tracing::warn!("Skipping malformed profile {}: {}", path.display(), e); }
            }
        }
    }
    Ok(profiles)
}

/// Generate default profile TOML files in the given directory.
pub fn generate_default_profiles(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, _desc, profile) in Profile::all() {
        let path = dir.join(format!("{}.toml", name));
        if !path.exists() {
            let def = ProfileDef { sampling: profile.params() };
            let toml_str = toml::to_string_pretty(&def)?;
            let comment = match name {
                "coding" => "# Sampling profile for code generation and agentic tasks.\n# Low temperature for deterministic, focused output.\n\n",
                "chat" => "# Sampling profile for conversational use.\n# Balanced temperature for natural responses.\n\n",
                "analysis" => "# Sampling profile for data analysis and reasoning.\n# Low temperature with focused sampling.\n\n",
                "creative" => "# Sampling profile for creative writing and brainstorming.\n# High temperature for diverse, exploratory output.\n\n",
                _ => "",
            };
            std::fs::write(&path, format!("{}{}", comment, toml_str))?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Update sampling resolution to use `profiles.d/`**

In `crates/kronk-core/src/config.rs`, update `effective_sampling_with_card`:

```rust
pub fn effective_sampling_with_card(
    &self,
    server: &ServerConfig,
    card: Option<&crate::models::card::ModelCard>,
) -> Option<SamplingParams> {
    // Layer 1: Profile base params (from profiles.d/ or built-in fallback)
    let base = match &server.profile {
        Some(Profile::Custom { name }) => self
            .custom_profiles
            .as_ref()
            .and_then(|m| m.get(name))
            .cloned(),
        Some(profile) => {
            // Try profiles.d/ first, fall back to built-in
            let from_disk = self.profiles_dir().ok()
                .and_then(|dir| crate::profiles::load_profiles_d(&dir).ok())
                .and_then(|map| map.get(&profile.to_string()).cloned());
            Some(from_disk.unwrap_or_else(|| profile.params()))
        }
        None => None,
    };
    // ... rest unchanged (layers 2 and 3)
}
```

- [ ] **Step 5: Generate default profiles on first run**

In `Config::load_from()`, after creating the default config:

```rust
// After writing default config:
let profiles_dir = config_dir.join("profiles.d");
if !profiles_dir.exists() {
    if let Err(e) = crate::profiles::generate_default_profiles(&profiles_dir) {
        tracing::warn!("Failed to generate default profiles: {}", e);
    }
}
```

- [ ] **Step 6: Update `cmd_profile` (was `cmd_use_case`) to show profiles from disk**

In the `List` handler, show profiles loaded from `profiles.d/`:

```rust
ProfileCommands::List => {
    let profiles_dir = config.profiles_dir()?;
    let disk_profiles = crate::profiles::load_profiles_d(&profiles_dir).unwrap_or_default();

    println!("Available profiles:");
    println!();

    for (name, desc, profile) in Profile::all() {
        let params = disk_profiles.get(name)
            .cloned()
            .unwrap_or_else(|| profile.params());
        println!("  {}:", name);
        println!("    {}", desc);
        // ... print params
        if disk_profiles.contains_key(name) {
            println!("    (loaded from profiles.d/{}.toml)", name);
        }
        println!();
    }

    // Show additional custom profiles from disk that aren't built-in
    for (name, params) in &disk_profiles {
        if !Profile::all().iter().any(|(n, _, _)| *n == name.as_str()) {
            println!("  {} (custom):", name);
            // ... print params
        }
    }
    // ...
}
```

- [ ] **Step 7: Write tests for `load_profiles_d` and `generate_default_profiles`**

In `crates/kronk-core/src/profiles.rs`:

```rust
#[test]
fn test_load_profiles_d_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles = load_profiles_d(tmp.path()).unwrap();
    assert!(profiles.is_empty());
}

#[test]
fn test_load_profiles_d_nonexistent() {
    let profiles = load_profiles_d(Path::new("/tmp/nonexistent_profiles_d_test")).unwrap();
    assert!(profiles.is_empty());
}

#[test]
fn test_generate_and_load_default_profiles() {
    let tmp = tempfile::tempdir().unwrap();
    generate_default_profiles(tmp.path()).unwrap();

    let profiles = load_profiles_d(tmp.path()).unwrap();
    assert!(profiles.contains_key("coding"));
    assert!(profiles.contains_key("chat"));
    assert!(profiles.contains_key("analysis"));
    assert!(profiles.contains_key("creative"));

    let coding = &profiles["coding"];
    assert_eq!(coding.temperature, Some(0.3));
}

#[test]
fn test_generate_does_not_overwrite_existing() {
    let tmp = tempfile::tempdir().unwrap();
    generate_default_profiles(tmp.path()).unwrap();

    let coding_path = tmp.path().join("coding.toml");
    std::fs::write(&coding_path, "[sampling]\ntemperature = 0.1\n").unwrap();

    generate_default_profiles(tmp.path()).unwrap();

    let profiles = load_profiles_d(tmp.path()).unwrap();
    assert_eq!(profiles["coding"].temperature, Some(0.1));
}
```

- [ ] **Step 8: Build and test**

```bash
cargo build --workspace && cargo test --workspace
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: ship default sampling profiles in profiles.d/ as editable TOML files"
```

---

## Task 6: Update README and user-facing documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update README**

Rewrite README sections to reflect new nomenclature:
- "Profile" → "Server" in Quick Start, CLI reference
- "Use case" → "Profile" in configuration examples
- Update CLI reference table completely
- Update config.toml example with `[servers.default]`, `profile = "coding"`, `enabled = true`
- Add `profiles.d/` and `configs.d/` to Architecture section
- Update directory structure diagram:

```text
~/.config/kronk/
├── config.toml
├── profiles.d/
│   ├── coding.toml
│   ├── chat.toml
│   ├── analysis.toml
│   └── creative.toml
├── configs.d/
│   └── bartowski--OmniCoder-8B.toml
├── models/
│   └── bartowski/OmniCoder-8B/*.gguf
└── logs/
```

- [ ] **Step 2: Build final verification**

```bash
cargo build --workspace && cargo test --workspace
```

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "docs: update README for server/profile nomenclature and new directory layout"
```

---

## Task 7: Verify community model card compatibility

**Files:**
- Read: `crates/kronk-core/src/models/pull.rs`

- [ ] **Step 1: Verify `fetch_community_card()` still works**

The community cards in the repo (`modelcards/`) use `[sampling.coding]` etc. — these key names don't change (profiles are still named "coding", "chat", etc.). The fetch function downloads from GitHub and the local save path changes to `configs.d/`. Verify the `cmd_pull` changes from Task 4 handle this correctly.

- [ ] **Step 2: Build and test final**

```bash
cargo build --workspace && cargo test --workspace
```

- [ ] **Step 3: Commit if any changes**

```bash
git add -A
git commit -m "chore: verify community model card compatibility with new layout"
```

---

## Summary of Breaking Changes

1. **Config TOML format:** `[profiles.x]` → `[servers.x]`, `use_case` → `profile`, `custom_use_cases` → `custom_profiles`
2. **CLI commands:** `kronk profile` → `kronk server`, `kronk use-case` → `kronk profile`
3. **CLI flags:** `kronk run --profile x` → `kronk run <name>` (positional, required)
4. **Service commands:** `--profile` flag → positional `<name>` (optional, defaults to all enabled)
5. **Model cards:** moved from `models/<company>/<model>/model.toml` → `configs.d/<company>--<model>.toml`
6. **New field:** `enabled = true` on servers (defaults to true for backward compat via `#[serde(default = "default_enabled")]`)
7. **New directory:** `profiles.d/` with editable sampling preset TOML files

Users with existing configs will need to manually update their `config.toml` (or we add a config migration in a follow-up).
