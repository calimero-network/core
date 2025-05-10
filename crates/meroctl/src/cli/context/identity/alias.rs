use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::GetContextIdentitiesResponse;
use clap::Parser;
use eyre::{OptionExt, Result as EyreResult, WrapErr};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    create_alias, delete_alias, fetch_multiaddr, load_config, lookup_alias, multiaddr_to_url,
    resolve_alias,
};
use crate::output::ErrorLine;

// Helper function needed by the Add subcommand implementation
async fn identity_exists_in_context(
    multiaddr: &Multiaddr,
    keypair: &Keypair,
    context: &Alias<ContextId>,
    target_identity: &PublicKey,
) -> EyreResult<bool> {
    let context_id = resolve_alias(multiaddr, keypair, *context, None)
        .await?
        .value()
        .cloned()
        .ok_or_eyre("unable to resolve alias")?;

    let endpoint = format!("admin-api/dev/contexts/{}/identities", context_id);
    let url = multiaddr_to_url(multiaddr, &endpoint)?;

    let response: GetContextIdentitiesResponse = Client::new()
        .get(url)
        .send()
        .await?
        .json::<GetContextIdentitiesResponse>() // Use the imported type directly
        .await?;

    Ok(response.data.identities.contains(target_identity))
}

#[derive(Debug, Parser)]
pub struct ContextIdentityAliasCommand {
    #[command(subcommand)]
    command: ContextIdentityAliasSubcommand,
}

#[derive(Debug, Parser)]
pub enum ContextIdentityAliasSubcommand {
    #[command(about = "Add new alias for an identity in a context", aliases = ["new", "create"])]
    Add {
        #[arg(help = "Name for the alias")]
        name: Alias<PublicKey>,

        #[arg(help = "The identity to create an alias for")]
        identity: PublicKey,

        #[arg(help = "The context that the identity is a member of")]
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
        #[arg(long, short, help = "Force overwrite if alias already exists")]
        force: bool,
    },

    #[command(
        about = "Remove an identity alias from a context",
        aliases = ["rm", "del", "delete"]
    )]
    Remove {
        #[arg(help = "Name of the alias to remove")]
        identity: Alias<PublicKey>,

        #[arg(help = "The context that the identity is a member of ")]
        #[arg(long, short)]
        context: Alias<ContextId>,
    },

    #[command(about = "Resolve the alias to a context identity")]
    Get {
        #[arg(help = "Name of the alias to look up")]
        identity: Alias<PublicKey>,

        #[arg(help = "The context that the identity is a member of ")]
        #[arg(long, short)]
        context: Alias<ContextId>,
    },
}

impl ContextIdentityAliasCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;

        match self.command {
            ContextIdentityAliasSubcommand::Add {
                name,
                identity,
                context,
                force,
            } => {
                let resolve_response =
                    resolve_alias(multiaddr, &config.identity, context, None).await?;

                if !identity_exists_in_context(&multiaddr, &config.identity, &context, &identity)
                    .await?
                {
                    environment.output.write(&ErrorLine(&format!(
                        "Identity '{}' does not exist in context '{}'",
                        identity, context
                    )));
                    return Ok(());
                }

                let context_id = resolve_response
                    .value()
                    .cloned()
                    .ok_or_eyre("Failed to resolve context: no value found")?;

                let lookup_result =
                    lookup_alias(multiaddr, &config.identity, name, Some(context_id)).await?;

                if let Some(existing_identity) = lookup_result.data.value {
                    if existing_identity == identity {
                        environment.output.write(&ErrorLine(&format!(
                            "Alias '{}' already exists and points to '{}'. Use --force to overwrite.",
                            name,
                            existing_identity
                        )));
                        return Ok(());
                    }
                    if !force {
                        environment.output.write(&ErrorLine(&format!(
                            "Alias '{}' already exists and points to '{}'. Use --force to overwrite.",
                            name,
                            existing_identity
                        )));
                        return Ok(());
                    }
                    environment.output.write(&ErrorLine(&format!(
                        "Overwriting existing alias '{}' from '{}' to '{}'",
                        name, existing_identity, identity
                    )));
                    let _ = delete_alias(multiaddr, &config.identity, name, Some(context_id))
                        .await
                        .wrap_err("Failed to delete existing alias")?;
                }

                let res = create_alias(
                    multiaddr,
                    &config.identity,
                    name,
                    Some(context_id),
                    identity,
                )
                .await?;

                environment.output.write(&res);
            }
            ContextIdentityAliasSubcommand::Remove { identity, context } => {
                let resolve_response =
                    resolve_alias(multiaddr, &config.identity, context, None).await?;

                let context_id = resolve_response
                    .value()
                    .cloned()
                    .ok_or_eyre("Failed to resolve context: no value found")?;
                let res =
                    delete_alias(multiaddr, &config.identity, identity, Some(context_id)).await?;

                environment.output.write(&res);
            }
            ContextIdentityAliasSubcommand::Get { identity, context } => {
                let resolve_response =
                    resolve_alias(multiaddr, &config.identity, context, None).await?;

                let context_id = resolve_response
                    .value()
                    .cloned()
                    .ok_or_eyre("Failed to resolve context: no value found")?;
                let res =
                    lookup_alias(multiaddr, &config.identity, identity, Some(context_id)).await?;

                environment.output.write(&res);
            }
        }

        Ok(())
    }
}
