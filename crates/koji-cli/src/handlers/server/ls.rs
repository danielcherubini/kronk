use anyhow::Result;
use koji_core::config::Config;
use koji_core::db::OpenResult;
/// List all servers with status
pub async fn cmd_server_ls(config: &Config) -> Result<()> {
    let db_dir = koji_core::config::Config::config_dir()?;
    let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
    let model_configs = koji_core::db::load_model_configs(&conn)?;

    if model_configs.is_empty() {
        println!("No models configured.");
        println!();
        println!("Pull one: koji model pull <repo>");
        return Ok(());
    }

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    println!("Models:");
    println!("{}", "-".repeat(60));

    for (name, srv) in &model_configs {
        // Backend lookup kept for potential future use
        let _unused_backend = config.backends.get(&srv.backend);
        let profile_name = if let Some(sampling) = &srv.sampling {
            sampling.preset_label().to_string()
        } else if let Some(ref profile) = srv.profile {
            profile.clone()
        } else {
            "none".to_string()
        };

        let service_name = Config::service_name(name);
        let service_status = {
            #[cfg(target_os = "windows")]
            {
                koji_core::platform::windows::query_service(&service_name)
                    .unwrap_or_else(|_| "UNKNOWN".to_string())
            }
            #[cfg(target_os = "linux")]
            {
                koji_core::platform::linux::auto_query_service(&service_name)
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
