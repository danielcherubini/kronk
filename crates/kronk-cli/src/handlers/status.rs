//! Status command handler
//!
//! Handles `kronk status` for showing status of all servers.

use anyhow::Result;
use kronk_core::config::Config;

/// Show status of all servers
pub async fn cmd_status(config: &Config) -> Result<()> {
    println!("KRONK Status");
    println!("{}", "-".repeat(60));

    // Query VRAM locally
    if let Some(vram) = kronk_core::gpu::query_vram() {
        println!("  VRAM:     {} / {} MiB", vram.used_mib, vram.total_mib);
    }

    // Query DB once outside loop
    let db_active = Config::config_dir()
        .ok()
        .and_then(|dir| kronk_core::db::open(&dir).ok())
        .and_then(|r| kronk_core::db::queries::get_active_models(&r.conn).ok())
        .unwrap_or_default();

    // Models from config
    for (name, srv) in &config.models {
        // Check if there's an active DB entry for this model
        let db_entry = db_active.iter().find(|m| m.server_name == *name);

        let loaded_str = match db_entry {
            Some(active) => {
                let pid = active.pid;
                if kronk_core::proxy::process::is_process_alive(pid as u32) {
                    format!("true (pid: {}, port: {})", pid, active.port)
                } else {
                    format!("false (stale — pid {} no longer running)", pid)
                }
            }
            None => "false".to_string(),
        };

        let backend_path = config
            .backends
            .get(&srv.backend)
            .map(|b| b.path.as_str())
            .unwrap_or("???");

        println!();
        println!("  Model:    {}", name);
        println!("  Source:   {}", srv.source.as_deref().unwrap_or(""));
        println!("  Quant:    {}", srv.quant.as_deref().unwrap_or(""));
        println!(
            "  Profile:  {}",
            srv.profile
                .as_ref()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
        if let Some(ctx) = srv.context_length {
            println!("  Context:  {}", ctx);
        }
        println!("  Backend:  {} ({})", srv.backend, backend_path);
        println!("  Loaded:   {}", loaded_str);
    }

    println!();
    Ok(())
}
