use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextResponse;
use clap::Parser;
use eyre::Result as EyreResult;
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::{ApiError, Environment};
use crate::common::{
    create_alias, delete_alias, do_request, fetch_multiaddr, load_config, lookup_alias,
    multiaddr_to_url, RequestType,
};
use crate::output::ErrorLine;

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
                if !context_exists(&multiaddr, &config.identity, &context_id).await? {
                    environment.output.write(&ErrorLine(&format!(
                        "Context with ID '{}' does not exist",
                        context_id
                    )));
                    return Ok(());
                }
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

async fn context_exists(
    multiaddr: &Multiaddr,
    identity: &Keypair,
    target_id: &ContextId,
) -> EyreResult<bool> {
    let url = multiaddr_to_url(multiaddr, &format!("admin-api/dev/contexts/{}", target_id))?;

    let result = do_request::<_, GetContextResponse>(
        &Client::new(),
        url,
        None::<()>,
        identity,
        RequestType::Get,
    )
    .await;

    match result {
        Ok(_) => Ok(true),
        Err(err) => {
            if let Some(api_error) = err.downcast_ref::<ApiError>() {
                if api_error.status_code == 404 {
                    return Ok(false);
                }
            }
            Err(err)
        }
    }
}
