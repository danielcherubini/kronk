//! Server command handler
//!
//! Handles `kronk server ls/add/edit/rm` commands.
mod add;
mod edit;
mod ls;
mod rm;
// Re-export all public command functions so callers don't change.
// e.g. `handlers::server::cmd_server_add` still works.
pub use add::cmd_server_add;
use anyhow::Result;
pub use edit::cmd_server_edit;
use koji_core::config::Config;
pub use ls::cmd_server_ls;
pub use rm::cmd_server_rm;
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
pub(super) fn resolve_backend(config: &mut Config, exe_path: &str) -> Result<(String, String)> {
    use koji_core::config::BackendConfig;
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
        .find(|(_, b)| b.path.as_deref() == Some(&exe_str))
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
                    path: Some(exe_str.clone()),
                    default_args: vec![],
                    health_check_url: None,
                    version: None,
                },
            );
            key
        }
    };
    Ok((backend_key, exe_str))
}
