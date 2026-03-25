//! Server command handler
//!
//! Handles `kronk server ls/add/edit/rm` commands.

use anyhow::{Context, Result};
use kronk_core::config::Config;

/// Manage servers — list, add, edit, remove
pub async fn cmd_server(config: &Config, command: crate::cli::ServerCommands) -> Result<()> {
    match command {
        crate::cli::ServerCommands::Ls => cmd_server_ls(config).await,
        crate::cli::ServerCommands::Add { name, command } => {
            cmd_server_add(config, &name, command, false).await
        }
        crate::cli::ServerCommands::Edit { name, command } => {
            if !config.models.contains_key(&name) {
                anyhow::bail!(
                    "Server '{}' not found. Use `kronk server add` to create it.",
                    name
                );
            }
            cmd_server_edit(&mut config.clone(), &name, command).await
        }
        crate::cli::ServerCommands::Rm { name, force } => cmd_server_rm(config, &name, force),
    }
}

/// List all servers with status
pub async fn cmd_server_ls(config: &Config) -> Result<()> {
    if config.models.is_empty() {
        println!("No models configured.");
        println!();
        println!("Pull one: kronk model pull <repo>");
        return Ok(());
    }

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    println!("Models:");
    println!("{}", "-".repeat(60));

    for (name, srv) in &config.models {
        let _backend = config.backends.get(&srv.backend);
        let profile_name = srv
            .profile
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "none".to_string());

        let service_name = Config::service_name(name);
        let service_status = {
            #[cfg(target_os = "windows")]
            {
                kronk_core::platform::windows::query_service(&service_name)
                    .unwrap_or_else(|_| "UNKNOWN".to_string())
            }
            #[cfg(target_os = "linux")]
            {
                kronk_core::platform::linux::query_service(&service_name)
                    .unwrap_or_else(|_| "UNKNOWN".to_string())
            }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                let _ = &service_name;
                "N/A".to_string()
            }
        };

        // Use server's resolved health check config
        let health_check = config.resolve_health_check(srv);
        let health = if let Some(url) = health_check.url {
            match http_client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => "HEALTHY",
                _ => "DOWN",
            }
        } else {
            "N/A"
        };

        println!();
        println!("  {}  (backend: {})", name, srv.backend);
        println!(
            "    profile: {}  service: {}  health: {}",
            profile_name, service_status, health
        );

        if let Some(ref model) = srv.model {
            let quant = srv.quant.as_deref().unwrap_or("?");
            println!("    model: {} / {}", model, quant);
        }

        if !srv.args.is_empty() {
            let args_str = srv.args.join(" ");
            if args_str.len() > 80 {
                let chars: Vec<char> = args_str.chars().take(77).collect();
                println!("    args: {}...", chars.iter().collect::<String>());
            } else {
                println!("    args: {}", args_str);
            }
        }
    }

    println!();
    Ok(())
}

/// Remove a server
pub fn cmd_server_rm(config: &Config, name: &str, force: bool) -> Result<()> {
    if !config.models.contains_key(name) {
        anyhow::bail!("Server '{}' not found.", name);
    }

    // Check if a service is installed for this server
    let service_name = Config::service_name(name);
    let service_installed = {
        #[cfg(target_os = "windows")]
        {
            kronk_core::platform::windows::query_service(&service_name)
                .map(|s| s != "NOT_INSTALLED")
                .unwrap_or(true)
        }
        #[cfg(target_os = "linux")]
        {
            kronk_core::platform::linux::query_service(&service_name)
                .map(|s| s != "NOT_INSTALLED")
                .unwrap_or(true)
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = &service_name;
            false
        }
    };

    if service_installed {
        anyhow::bail!(
            "Server '{}' has an installed service '{}'. Remove it first with: kronk service remove {}",
            name, service_name, name
        );
    }

    if !force {
        let confirm = inquire::Confirm::new(&format!("Remove model '{}'?", name))
            .with_default(false)
            .prompt()
            .context("Confirmation cancelled")?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let mut config = config.clone();
    config.models.remove(name);
    config.save()?;

    println!("Model '{}' removed.", name);
    Ok(())
}

/// Resolve a backend path to a backend key in the config.
///
/// This function handles:
/// - Path absolutization: filesystem paths (containing separators, starting with `./` or `/`)
///   are resolved to absolute paths; bare command names (e.g., "llama-server") are left as-is
///   for PATH resolution at runtime.
/// - Finding an existing backend by path, or creating a new one if not found.
///
/// # Arguments
/// * `config` - Mutable config to store new backends
/// * `exe_path` - The executable path or bare command name
///
/// # Returns
/// The backend key (name) that should be used for this backend.
fn resolve_backend(config: &mut Config, exe_path: &str) -> Result<(String, String)> {
    use kronk_core::config::BackendConfig;

    // Only absolutize if it looks like a filesystem path (contains separator or starts with ./..);
    // bare command names (e.g. "llama-server") are left as-is so PATH resolution works at runtime.
    let exe_abs = std::path::Path::new(exe_path);
    let is_path = exe_path.contains(std::path::MAIN_SEPARATOR)
        || exe_path.contains('/')
        || exe_path.starts_with('.')
        || exe_abs.is_absolute();
    let (exe_str, exe_stem) = if is_path {
        let resolved = if exe_abs.is_absolute() {
            exe_abs.to_path_buf()
        } else {
            std::env::current_dir()?.join(exe_abs)
        };
        let stem = resolved
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "backend".to_string());
        (resolved.to_string_lossy().to_string(), stem)
    } else {
        let stem = exe_path
            .strip_suffix(".exe")
            .unwrap_or(exe_path)
            .to_string();
        (exe_path.to_string(), stem)
    };

    // Check if this backend path already exists
    let backend_name = config
        .backends
        .iter()
        .find(|(_, b)| b.path == exe_str)
        .map(|(k, _)| k.clone());

    let backend_key = match backend_name {
        Some(k) => k,
        None => {
            // Derive a backend name from the exe filename, avoiding collisions
            let base = exe_stem.replace('-', "_");

            let mut key = base.clone();
            let mut i = 2;
            while config.backends.contains_key(&key) {
                key = format!("{}_{}", base, i);
                i += 1;
            }

            config.backends.insert(
                key.clone(),
                BackendConfig {
                    path: exe_str.clone(),
                    default_args: vec![],
                    health_check_url: None,
                },
            );
            key
        }
    };

    Ok((backend_key, exe_str))
}

/// Add a new server from a command line, extracting kronk flags.
pub async fn cmd_server_add(
    config: &Config,
    name: &str,
    command: Vec<String>,
    overwrite: bool,
) -> Result<()> {
    use anyhow::Context;

    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    // Split exe from args
    let exe_path = &command[0];
    let args: Vec<String> = command[1..].to_vec();

    // Resolve backend path
    let mut config = config.clone();
    let (backend_key, exe_str) = resolve_backend(&mut config, exe_path)?;

    // Extract kronk flags from args
    let extracted = crate::flags::extract_kronk_flags(args.clone())?;

    // Check for duplicate server
    if config.models.contains_key(name) && !overwrite {
        anyhow::bail!(
            "Server '{}' already exists. Use `kronk server edit` to modify it.",
            name
        );
    }

    // Resolve model card if model ref is provided
    let model_info = if let Some(ref model_ref) = extracted.model {
        let models_dir = config.models_dir()?;
        let configs_dir = config.configs_dir()?;
        let registry = kronk_core::models::ModelRegistry::new(
            models_dir.to_path_buf(),
            configs_dir.to_path_buf(),
        );

        match registry.find(model_ref) {
            Ok(Some(installed)) => Some(installed),
            Ok(None) => {
                anyhow::bail!(
                    "Model '{}' not found. Use `kronk model pull <repo>` to install it first.",
                    model_ref
                );
            }
            Err(e) => {
                anyhow::bail!("Failed to look up model '{}': {}", model_ref, e);
            }
        }
    } else {
        None
    };

    // Resolve quantization with interactive selection if needed
    let quant_name = if let Some(ref quant) = extracted.quant {
        // If model card exists, validate quant against available quants
        if let Some(ref installed) = model_info {
            if !installed.card.quants.contains_key(quant) {
                let available: Vec<_> = installed.card.quants.keys().collect();
                anyhow::bail!(
                    "Quant '{}' not found in model '{}'. Available quants: {}",
                    quant,
                    installed.card.model.source,
                    available
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Some(quant.clone())
        } else {
            // No model card - quant is just stored as-is
            Some(quant.clone())
        }
    } else if let Some(ref installed) = model_info {
        // Model ref provided but no --quant - offer interactive selection
        let quant_names: Vec<String> = installed.card.quants.keys().cloned().collect();
        if quant_names.is_empty() {
            anyhow::bail!(
                "No quants available for '{}'. Run `kronk model pull {}` first.",
                installed.card.model.source,
                installed.card.model.source
            );
        }
        Some(if quant_names.len() == 1 {
            quant_names.into_iter().next().unwrap()
        } else {
            inquire::Select::new("Select a quant:", quant_names)
                .prompt()
                .context("Quant selection cancelled")?
        })
    } else {
        None
    };

    // Verify GGUF file exists if both model card and quant are specified
    if let Some(ref quant_name) = quant_name {
        if let Some(ref installed) = model_info {
            let gguf_path = installed
                .card
                .quants
                .get(quant_name)
                .map(|q| installed.dir.join(&q.file));

            if let Some(ref path) = gguf_path {
                if !path.exists() {
                    anyhow::bail!(
                        "GGUF file '{}' not found. Make sure the model is properly installed.",
                        path.display()
                    );
                }
            }
        }
    }

    // Parse profile if provided
    let profile = extracted
        .profile
        .as_ref()
        .map(|s| {
            s.parse::<kronk_core::profiles::Profile>()
                .map_err(|e| anyhow::anyhow!(e))
        })
        .transpose()?;

    // Build ModelConfig
    let model_config = kronk_core::config::ModelConfig {
        backend: backend_key.clone(),
        args: extracted.remaining_args.clone(),
        profile,
        sampling: None,
        model: extracted.model.clone(),
        quant: quant_name,
        port: extracted.port,
        health_check: None,
        enabled: true,
        source: model_info.as_ref().map(|m| m.card.model.source.clone()),
        context_length: extracted.context_length,
    };

    config.models.insert(name.to_string(), model_config.clone());
    config.save()?;

    // Output
    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Model:    {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);

    if let Some(ref model) = model_config.model {
        let quant = model_config.quant.as_deref().unwrap_or("?");
        let source = model_config.source.as_deref().unwrap_or(model);
        println!("  Model:    {} ({})", source, quant);
    }

    if let Some(ref profile) = model_config.profile {
        println!("  Profile:  {}", profile);
    }

    if let Some(ref quant) = model_config.quant {
        if let Some(ref model) = model_config.model {
            let models_dir = config.models_dir()?;
            let configs_dir = config.configs_dir()?;
            let registry = kronk_core::models::ModelRegistry::new(
                models_dir.to_path_buf(),
                configs_dir.to_path_buf(),
            );
            if let Ok(Some(installed)) = registry.find(model) {
                if let Some(q) = installed.card.quants.get(quant) {
                    println!("  GGUF:     {}", q.file);
                }
            }
        }
    }

    println!();
    println!("Enable it:  kronk model enable {}", name);
    println!("Start:      kronk serve");

    Ok(())
}

/// Edit an existing server's command line
pub async fn cmd_server_edit(config: &mut Config, name: &str, command: Vec<String>) -> Result<()> {
    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    // Verify server exists before any mutations
    if !config.models.contains_key(name) {
        anyhow::bail!("Server '{}' not found", name);
    }

    let exe_path = &command[0];
    let args: Vec<String> = command[1..].to_vec();

    let (backend_key, exe_str) = resolve_backend(config, exe_path)?;

    // Extract kronk flags from args
    let extracted = crate::flags::extract_kronk_flags(args)?;

    // Mutate via get_mut in a block so the borrow is dropped before save()
    {
        let srv = config.models.get_mut(name).unwrap();

        // Selectively merge extracted flags into existing ModelConfig
        if let Some(ref model) = extracted.model {
            srv.model = Some(model.clone());
            srv.source = Some(model.clone());
        }
        if let Some(ref quant) = extracted.quant {
            srv.quant = Some(quant.clone());
        }
        if let Some(ref profile) = extracted.profile {
            let p = profile
                .parse::<kronk_core::profiles::Profile>()
                .map_err(|e| anyhow::anyhow!(e))?;
            srv.profile = Some(p);
        }
        if let Some(port) = extracted.port {
            srv.port = Some(port);
        }
        if let Some(ctx) = extracted.context_length {
            srv.context_length = Some(ctx);
        }

        srv.backend = backend_key.clone();
        srv.args = extracted.remaining_args.clone();
    }

    config.save()?;

    // Read back for output
    let srv = config.models.get(name).unwrap();

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Name:     {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);

    if let Some(ref model) = srv.model {
        let quant = srv.quant.as_deref().unwrap_or("?");
        let source = srv.source.as_deref().unwrap_or(model);
        println!("  Model:    {} ({})", source, quant);
    }
    if let Some(ref profile) = srv.profile {
        println!("  Profile:  {}", profile);
    }

    println!();

    Ok(())
}
