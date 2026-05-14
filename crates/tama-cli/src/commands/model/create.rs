use anyhow::{Context, Result};
use tama_core::config::Config;
use tama_core::models::ModelManager;

pub(super) async fn cmd_create(
    config: &Config,
    server_name_arg: Option<String>,
    model_id_arg: &str,
    quant_name: Option<String>,
    profile_name_arg: Option<String>,
    backend_arg: Option<String>,
) -> Result<()> {
    // Resolve server name — prompt if not provided
    let server_name = match server_name_arg {
        Some(n) => n,
        None => inquire::Text::new("Config name (e.g. gemma4-coding):")
            .prompt()
            .context("Config name input cancelled")?,
    };

    // Resolve DB
    let db_dir = tama_core::config::Config::config_dir()?;
    let mgr = ModelManager::open(&db_dir)?;

    // Resolve backend
    let resolved_backend_key = match backend_arg {
        Some(b) => {
            if !config.backends.contains_key(&b) {
                let available: Vec<&str> = config.backends.keys().map(|s| s.as_str()).collect();
                anyhow::bail!(
                    "Backend '{}' not found. Available: {}",
                    b,
                    available.join(", ")
                );
            }
            b
        }
        None => {
            let keys: Vec<String> = config.backends.keys().cloned().collect();
            match keys.len() {
                0 => anyhow::bail!("No backends configured. Add one first with `tama add`."),
                1 => keys.into_iter().next().unwrap(),
                _ => inquire::Select::new("Select a backend:", keys)
                    .prompt()
                    .context("Backend selection cancelled")?,
            }
        }
    };

    // Resolve profile
    let resolved_profile: Option<tama_core::profiles::Profile> = match profile_name_arg {
        Some(p) => Some(
            p.parse::<tama_core::profiles::Profile>()
                .map_err(|e| anyhow::anyhow!(e))?,
        ),
        None => None,
    };

    let model_config = tama_core::config::ModelConfig {
        backend: resolved_backend_key.clone(),
        gpu_variant: None,
        args: vec![],
        profile: resolved_profile.map(|p| p.to_string()),
        sampling: None,
        model: Some(model_id_arg.to_string()),
        quant: quant_name.clone(),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        api_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        num_parallel: None,
        kv_unified: true,
        cache_type_k: None,
        cache_type_v: None,
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        db_id: None,
    };

    mgr.save_model_config(&server_name, &model_config)?;

    println!("Created.");
    println!();
    println!("  Name:      {}", server_name);
    println!("  Model:     {}", model_id_arg);
    if let Some(ref q) = quant_name {
        println!("  Quant:     {}", q);
    }
    if let Some(sampling) = &model_config.sampling {
        println!("  Profile:   {}", sampling.preset_label());
    } else if let Some(p) = &model_config.profile {
        println!("  Profile:   {}", p);
    }
    println!();
    println!("Enable it:   tama model enable {}", server_name);
    println!("Start:       tama serve");

    Ok(())
}
