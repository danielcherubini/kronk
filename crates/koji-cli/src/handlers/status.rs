//! Status command handler
//!
//! Handles `kronk status` for showing status of all servers.
//! Queries the proxy API when available for rich runtime info,
//! falls back to DB-based status when the proxy is unreachable.

use anyhow::Result;
use koji_core::config::Config;

/// Show status of all servers
pub async fn cmd_status(config: &Config) -> Result<()> {
    println!("KRONK Status");
    println!("{}", "-".repeat(60));

    // Try the proxy API first — it has the richest info
    let proxy_url = config.proxy_url();
    let status_url = format!("{}/status", proxy_url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;

    match client.get(&status_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await?;
            print_proxy_status(&body, config);
        }
        _ => {
            // Proxy unreachable — fall back to config + DB
            println!("  Proxy:    offline ({})", proxy_url);
            print_offline_status(config);
        }
    }

    println!();
    Ok(())
}

/// Print status from the proxy API response (rich runtime info).
fn print_proxy_status(status: &serde_json::Value, config: &Config) {
    // VRAM
    if let Some(vram) = status.get("vram").and_then(|v| v.as_object()) {
        let used = vram.get("used_mib").and_then(|v| v.as_u64()).unwrap_or(0);
        let total = vram.get("total_mib").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("  VRAM:     {} / {} MiB", used, total);
    }

    // Idle timeout
    if let Some(timeout) = status.get("idle_timeout_secs").and_then(|v| v.as_u64()) {
        println!("  Timeout:  {}s", timeout);
    }

    // Metrics
    if let Some(metrics) = status.get("metrics").and_then(|v| v.as_object()) {
        let total = metrics
            .get("total_requests")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let loaded = metrics
            .get("models_loaded")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let unloaded = metrics
            .get("models_unloaded")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if total > 0 || loaded > 0 || unloaded > 0 {
            println!(
                "  Requests: {} total, {} loaded, {} unloaded",
                total, loaded, unloaded
            );
        }
    }

    // Resolve context lengths from model cards for models that don't have config overrides
    let model_contexts = resolve_model_contexts(config);

    // Models
    if let Some(models) = status.get("models").and_then(|v| v.as_object()) {
        for (name, model) in models {
            println!();
            println!("  Model:    {}", name);

            // HF model reference
            if let Some(model_ref) = model.get("model").and_then(|v| v.as_str()) {
                println!("  HF Ref:   {}", model_ref);
            }

            if let Some(quant) = model.get("quant").and_then(|v| v.as_str()) {
                println!("  Quant:    {}", quant);
            }

            // Context length — from API (config override) or resolved from model card
            let ctx = model
                .get("context_length")
                .and_then(|v| v.as_u64())
                .or_else(|| model_contexts.get(name.as_str()).copied());
            if let Some(ctx) = ctx {
                println!("  Context:  {}", ctx);
            }

            if let Some(profile) = model.get("profile").and_then(|v| v.as_str()) {
                println!("  Profile:  {}", profile);
            }

            let backend = model.get("backend").and_then(|v| v.as_str()).unwrap_or("?");
            let backend_path = model
                .get("backend_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            println!("  Backend:  {} ({})", backend, backend_path);

            let loaded = model
                .get("loaded")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let enabled = model
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            if !enabled {
                println!("  Status:   disabled");
                continue;
            }

            if loaded {
                let pid = model.get("backend_pid").and_then(|v| v.as_u64());
                let mut parts = vec!["loaded".to_string()];
                if let Some(pid) = pid {
                    parts.push(format!("pid: {}", pid));
                }
                println!("  Status:   {}", parts.join(", "));

                // Last accessed
                if let Some(secs) = model.get("last_accessed_secs_ago").and_then(|v| v.as_u64()) {
                    println!("  Accessed: {}s ago", secs);
                }

                // Idle timeout remaining
                if let Some(remaining) = model
                    .get("idle_timeout_remaining_secs")
                    .and_then(|v| v.as_u64())
                {
                    println!("  Idle in:  {}s", remaining);
                }

                // Load time
                if let Some(load_ts) = model.get("load_time_secs").and_then(|v| v.as_u64()) {
                    if load_ts > 0 {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let ago = now.saturating_sub(load_ts);
                        println!("  Loaded:   {}s ago", ago);
                    }
                }

                // Consecutive failures
                if let Some(failures) = model.get("consecutive_failures").and_then(|v| v.as_u64()) {
                    if failures > 0 {
                        println!("  Failures: {}", failures);
                    }
                }
            } else {
                println!("  Status:   idle");
            }
        }
    }
}

/// Resolve context lengths from model cards for display.
fn resolve_model_contexts(config: &Config) -> std::collections::HashMap<&str, u64> {
    let mut contexts = std::collections::HashMap::new();

    let models_dir = config.models_dir().ok();
    let configs_dir = config
        .configs_dir()
        .ok()
        .or_else(|| models_dir.as_ref().map(|_| config.models_dir().unwrap()));

    if let (Some(models_dir), Some(configs_dir)) = (models_dir, configs_dir) {
        let registry =
            koji_core::models::ModelRegistry::new(models_dir.clone(), configs_dir.clone());

        for (name, srv) in &config.models {
            // If config already has a context_length, use that
            if let Some(ctx) = srv.context_length {
                contexts.insert(name.as_str(), ctx as u64);
                continue;
            }
            // Otherwise resolve from model card
            if let Some(ref model_id) = srv.model {
                if let Ok(Some(installed)) = registry.find(model_id) {
                    let quant_name = srv.quant.as_deref().unwrap_or("");
                    if let Some(ctx) = installed.card.context_length_for(quant_name) {
                        contexts.insert(name.as_str(), ctx as u64);
                    }
                }
            }
        }
    }
    contexts
}

/// Fallback: print status from config + DB when proxy is offline.
fn print_offline_status(config: &Config) {
    if let Some(vram) = koji_core::gpu::query_vram() {
        println!("  VRAM:     {} / {} MiB", vram.used_mib, vram.total_mib);
    }

    let db_active = Config::config_dir()
        .ok()
        .and_then(|dir| koji_core::db::open(&dir).ok())
        .and_then(|r| koji_core::db::queries::get_active_models(&r.conn).ok())
        .unwrap_or_default();

    let model_contexts = resolve_model_contexts(config);

    for (name, srv) in &config.models {
        let db_entry = db_active.iter().find(|m| m.server_name == *name);

        let status_str = match db_entry {
            Some(active) => {
                let pid = active.pid;
                if koji_core::proxy::process::is_process_alive(pid as u32) {
                    format!("loaded (pid: {}, port: {})", pid, active.port)
                } else {
                    format!("stale (pid {} no longer running)", pid)
                }
            }
            None => "idle".to_string(),
        };

        let backend_path = config
            .backends
            .get(&srv.backend)
            .and_then(|b| b.path.as_deref())
            .unwrap_or("???");

        println!();
        println!("  Model:    {}", name);
        if let Some(ref model_ref) = srv.model {
            println!("  HF Ref:   {}", model_ref);
        }
        if let Some(ref quant) = srv.quant {
            println!("  Quant:    {}", quant);
        }
        if let Some(ctx) = srv
            .context_length
            .map(|c| c as u64)
            .or_else(|| model_contexts.get(name.as_str()).copied())
        {
            println!("  Context:  {}", ctx);
        }
        println!(
            "  Profile:  {}",
            if let Some(sampling) = &srv.sampling {
                sampling.preset_label().to_string()
            } else if let Some(ref profile) = srv.profile {
                profile.clone()
            } else {
                "none".to_string()
            }
        );
        println!("  Backend:  {} ({})", srv.backend, backend_path);
        if !srv.enabled {
            println!("  Status:   disabled");
        } else {
            println!("  Status:   {}", status_str);
        }
    }
}
