use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::{Result as EyreResult, WrapErr};

use crate::cli::Environment;
use crate::common::{create_alias, delete_alias, fetch_multiaddr, load_config, lookup_alias};

#[derive(Debug, Parser)]
#[command(about = "Manage context aliases")]
pub struct ContextAliasCommand {
    #[command(subcommand)]
    command: ContextAliasSubcommand,
}

#[derive(Debug, Parser)]
pub enum ContextAliasSubcommand {
    #[command(about = "Add new alias for a context", aliases = ["new", "create"])]
    Add {
        #[arg(help = "Name for the alias")]
        alias: Alias<ContextId>,

        #[arg(help = "The context to create an alias for")]
        context_id: ContextId,
    },

    #[command(about = "Remove a context alias", aliases = ["rm", "del", "delete"])]
    Remove {
        #[arg(help = "Name of the alias to remove")]
        alias: Alias<ContextId>,
    },

    #[command(about = "Resolve the alias to a context")]
    Get {
        #[arg(help = "Name of the alias to look up")]
        alias: Alias<ContextId>,
    },
}

impl ContextAliasCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;

        match self.command {
            ContextAliasSubcommand::Add { alias, context_id } => {
                let res =
                    create_alias(multiaddr, &config.identity, alias, None, context_id).await?;

                environment.output.write(&res);
            }
            ContextAliasSubcommand::Remove { alias } => {
                let res = delete_alias(multiaddr, &config.identity, alias, None).await?;

                environment.output.write(&res);
            }
            ContextAliasSubcommand::Get { alias } => {
                let res = lookup_alias(multiaddr, &config.identity, alias, None).await?;

                environment.output.write(&res);
            }
        }

        Ok(())
    }
}

#[derive(Debug, Parser)]
#[command(name = "use", about = "Set the default context")]
pub struct UseCommand {
    /// The context id to set as default
    pub context_id: ContextId,
}

impl UseCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;

        // Create "default" alias for the specified context ID
        let default_alias: Alias<ContextId> =
            "default".parse().expect("'default' is a valid alias name");
        let res = create_alias(
            multiaddr,
            &config.identity,
            default_alias,
            None,
            self.context_id,
        )
        .await
        .wrap_err("Failed to set default context")?;
        environment.output.write(&res);

        println!("Default context set to: {}", self.context_id);
        Ok(())
    }
}
