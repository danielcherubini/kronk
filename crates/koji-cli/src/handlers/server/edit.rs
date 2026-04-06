use anyhow::Result;
use koji_core::config::Config;
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

    let (backend_key, exe_str) = super::resolve_backend(config, exe_path)?;

    // Extract kronk flags from args
    let extracted = crate::flags::extract_kronk_flags(args)?;

    // Mutate via get_mut in a block so the borrow is dropped before save()
    {
        let srv = config.models.get_mut(name).unwrap();

        // Selectively merge extracted flags into existing ModelConfig
        if let Some(ref model) = extracted.model {
            srv.model = Some(model.clone());
        }
        if let Some(ref quant) = extracted.quant {
            srv.quant = Some(quant.clone());
        }
        if let Some(ref profile) = extracted.profile {
            // Set profile for migration compatibility
            srv.profile = Some(profile.clone());
            // Look up sampling template
            if let Some(template) = config.sampling_templates.get(profile) {
                srv.sampling = Some(template.clone());
            }
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

    println!("Server updated successfully.");
    println!();
    println!("  Name:     {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);

    if let Some(ref model) = srv.model {
        let quant = srv.quant.as_deref().unwrap_or("?");
        println!("  Model:    {} ({})", model, quant);
    }
    if let Some(sampling) = &srv.sampling {
        // Show which profile was used based on sampling values
        if sampling.temperature == Some(0.3)
            && sampling.top_p == Some(0.9)
        {
            println!("  Profile:  coding");
        } else if sampling.temperature == Some(0.7)
            && sampling.top_p == Some(0.95)
        {
            println!("  Profile:  chat");
        } else if sampling.temperature == Some(0.2)
            && sampling.top_p == Some(0.5)
        {
            println!("  Profile:  analysis");
        } else if sampling.temperature == Some(0.9)
            && sampling.top_p == Some(0.95)
        {
            println!("  Profile:  creative");
        } else {
            println!("  Profile:  custom");
        }
    }

    println!();

    Ok(())
}
