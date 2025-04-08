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
#[command(about = "Set the default context")]
pub struct UseCommand {
    pub context_id: Alias<ContextId>,
}

impl UseCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;

        // Create "default" alias for the specified context ID
        let default_alias: Alias<ContextId> =
            "default".parse().expect("'default' is a valid alias name");

        // Resolve the input (whether it's an alias or a context ID)
        let lookup_response =
            lookup_alias(multiaddr, &config.identity, self.context_id, None)
                .await
                .wrap_err("Failed to resolve context alias")?;

        // Extract the context ID from the lookup response
        let resolved_context_id = lookup_response
            .data
            .value
            .ok_or_else(|| eyre::eyre!("Failed to resolve context: no value found"))?;

        // Set the "default" alias to point to the resolved context ID
        let res = create_alias(
            multiaddr,
            &config.identity,
            default_alias,
            None,
            resolved_context_id.clone(),
        )
        .await
        .wrap_err("Failed to set default context")?;

        environment.output.write(&res);

        // Check if input was likely an alias or a direct context ID
        let input = self.context_id.to_string();
        if input == resolved_context_id.to_string() {
            println!("Default context set to: {}", resolved_context_id);
        } else {
            println!(
                "Default context set to: {} (from alias '{}')",
                resolved_context_id, input
            );
        }

        Ok(())
    }
}
