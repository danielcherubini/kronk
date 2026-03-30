//! Status command handler
//!
//! Handles `kronk status` for showing status of all servers.

use anyhow::Result;
use kronk_core::config::Config;

/// Show status of all servers
pub async fn cmd_status(config: &Config) -> Result<()> {
    println!("KRONK Status");
    println!("{}", "-".repeat(60));

    // Query proxy /status endpoint with 500ms timeout
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()
        .unwrap_or_default();

    let proxy_url = config.proxy_url();
    let proxy_response = if !proxy_url.is_empty() {
        match http_client
            .get(format!("{}/status", proxy_url))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => resp.json::<serde_json::Value>().await.ok(),
            _ => None,
        }
    } else {
        None
    };

    if let Some(ref proxy_json) = proxy_response {
        // VRAM from proxy response
        if let Some(vram) = proxy_json.get("vram").and_then(|v| v.as_object()) {
            let used = vram.get("used_mib").and_then(|v| v.as_u64()).unwrap_or(0);
            let total = vram.get("total_mib").and_then(|v| v.as_u64()).unwrap_or(0);
            println!("  VRAM:     {} / {} MiB", used, total);
        }

        // Models from proxy response (object keyed by model name)
        if let Some(models) = proxy_json.get("models").and_then(|m| m.as_object()) {
            for (model_name, model) in models {
                let backend = model
                    .get("backend")
                    .and_then(|v| v.as_str())
                    .unwrap_or("???");
                let backend_path = model
                    .get("backend_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("???");
                let source = model.get("source").and_then(|v| v.as_str()).unwrap_or("");
                let quant = model.get("quant").and_then(|v| v.as_str()).unwrap_or("");
                let profile = model.get("profile").and_then(|v| v.as_str()).unwrap_or("");
                let context_length = model.get("context_length").and_then(|v| v.as_u64());
                let loaded = model
                    .get("loaded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let backend_pid = model
                    .get("backend_pid")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .unwrap_or(0);

                let loaded_str = if loaded {
                    let last_accessed = model
                        .get("last_accessed_secs_ago")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let remaining = model
                        .get("idle_timeout_remaining_secs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    format!(
                        "true (pid: {}, idle: {}s ago, unloads in {})",
                        backend_pid,
                        last_accessed,
                        format_duration_secs(remaining),
                    )
                } else {
                    "false".to_string()
                };

                println!();
                println!("  Model:    {}", model_name);
                println!("  Source:   {}", source);
                println!("  Quant:    {}", quant);
                println!("  Profile:  {}", profile);
                if let Some(ctx) = context_length {
                    println!("  Context:  {}", ctx);
                }
                println!("  Backend:  {} ({})", backend, backend_path);
                println!("  Loaded:   {}", loaded_str);
            }
        }
    } else {
        // Proxy not running - query VRAM locally for fallback
        if let Some(vram) = kronk_core::gpu::query_vram() {
            println!("  VRAM:     {} / {} MiB", vram.used_mib, vram.total_mib);
        }

        // Query active_models from DB for fallback status
        let db_active = Config::config_dir()
            .ok()
            .and_then(|dir| kronk_core::db::open(&dir).ok())
            .and_then(|r| kronk_core::db::queries::get_active_models(&r.conn).ok())
            .unwrap_or_default();

        for (name, srv) in &config.models {
            // Check if there's an active DB entry for this model
            let db_entry = db_active.iter().find(|m| m.model_name == *name);

            let loaded_str = match db_entry {
                Some(active) => {
                    let pid = active.pid;
                    if kronk_core::proxy::process::is_process_alive(pid as u32) {
                        if active.port > 0 {
                            format!("true (pid: {}, port: {})", pid, active.port)
                        } else {
                            format!("true (pid: {})", pid)
                        }
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
    }

    println!();
    Ok(())
}

/// Format seconds as human-readable duration (e.g. "4m28s" or "32s").
fn format_duration_secs(secs: u64) -> String {
    if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}
