pub mod create;
pub mod enable_disable;
pub mod list_rm;
pub mod migrate;
pub mod prune;
pub mod pull;
pub mod update;
pub mod utils;
pub mod verify;

pub use crate::cli::ModelCommands;

pub async fn run(config: &koji_core::config::Config, command: ModelCommands) -> anyhow::Result<()> {
    match command {
        ModelCommands::Pull { repo } => pull::cmd_pull(config, &repo).await,
        ModelCommands::Ls {
            model,
            quant,
            profile,
        } => list_rm::cmd_ls(config, model, quant, profile),
        ModelCommands::Enable { name } => enable_disable::cmd_enable(config, &name),
        ModelCommands::Disable { name } => enable_disable::cmd_disable(config, &name),
        ModelCommands::Create {
            name,
            model,
            quant,
            profile,
            backend,
        } => create::cmd_create(config, name, &model, quant, profile, backend).await,
        ModelCommands::Rm { model } => list_rm::cmd_rm(config, &model),
        ModelCommands::Scan => pull::cmd_scan(config),
        ModelCommands::Prune { dry_run, yes } => prune::cmd_prune(config, dry_run, yes),
        ModelCommands::Update {
            model,
            check,
            refresh,
            yes,
        } => update::cmd_update(config, model, check, refresh, yes).await,
        ModelCommands::Search {
            query,
            sort,
            limit,
            pull,
        } => update::cmd_search(config, &query, &sort, limit, pull).await,
        ModelCommands::Verify { model } => match verify::cmd_verify(config, model).await {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        },
        ModelCommands::VerifyExisting { model, verbose } => {
            match verify::cmd_verify_existing(config, model, verbose).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            }
        }
        ModelCommands::Migrate => migrate::cmd_migrate(config),
    }
}
