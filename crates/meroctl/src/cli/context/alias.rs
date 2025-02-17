use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::Result as EyreResult;

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
    #[command(about = "Add new alias for a context", alias = "create")]
    Add {
        #[arg(help = "Alias name")]
        alias: Alias<ContextId>,

        #[arg(help = "Context hash")]
        context_id: ContextId,
    },

    #[command(about = "Remove a context alias", alias = "delete", alias = "rm")]
    Remove {
        #[arg(help = "Alias name")]
        alias: Alias<ContextId>,
    },

    #[command(about = "Get the hash attached to a context alias")]
    Get {
        #[arg(help = "Alias name")]
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
