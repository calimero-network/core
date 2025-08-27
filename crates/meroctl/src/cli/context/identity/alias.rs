use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::Parser;
use eyre::{OptionExt, Result, WrapErr};

use crate::cli::Environment;
use crate::output::ErrorLine;

#[derive(Copy, Clone, Debug, Parser)]
pub struct ContextIdentityAliasCommand {
    #[command(subcommand)]
    pub command: ContextIdentityAliasSubcommand,
}

#[derive(Copy, Clone, Debug, Parser)]
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

        #[arg(help = "The context that the identity is a member of")]
        #[arg(long, short)]
        context: Alias<ContextId>,
    },

    #[command(about = "Resolve the alias to a context identity")]
    Get {
        #[arg(help = "Name of the alias to look up", default_value = "default")]
        identity: Alias<PublicKey>,

        #[arg(help = "The context that the identity is a member of")]
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
    },

    #[command(about = "List all the aliases under the context", alias = "ls")]
    List {
        #[arg(help = "The context whose aliases need to be listed")]
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
    },
}

impl ContextIdentityAliasCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.mero_client()?.clone();

        // Extract context and resolve it to context_id
        let context_id = match &self.command {
            ContextIdentityAliasSubcommand::Add { context, .. }
            | ContextIdentityAliasSubcommand::Remove { context, .. }
            | ContextIdentityAliasSubcommand::Get { context, .. }
            | ContextIdentityAliasSubcommand::List { context } => client
                .resolve_alias(*context, None)
                .await?
                .value()
                .cloned()
                .ok_or_eyre("Failed to resolve context: no value found")?,
        };

        match self.command {
            ContextIdentityAliasSubcommand::Add {
                name,
                identity,
                context,
                force,
            } => {
                // Check if identity exists in context using MeroClient
                let response = client.get_context_identities(&context_id, false).await?;
                let identity_exists = response.data.identities.contains(&identity);

                if !identity_exists {
                    environment.output.write(&ErrorLine(&format!(
                        "Identity '{}' does not exist in context '{}'",
                        identity, context
                    )));
                    return Ok(());
                }

                let lookup_result = client.lookup_alias(name, Some(context_id)).await?;

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
                    let _ignored = client
                        .delete_alias(name, Some(context_id))
                        .await
                        .wrap_err("Failed to delete existing alias")?;
                }

                let res = client
                    .create_alias_generic(name, Some(context_id), identity)
                    .await?;

                environment.output.write(&res);
            }
            ContextIdentityAliasSubcommand::Remove {
                identity,
                context: _,
            } => {
                let res = client.delete_alias(identity, Some(context_id)).await?;

                environment.output.write(&res);
            }
            ContextIdentityAliasSubcommand::Get {
                identity,
                context: _,
            } => {
                let res = client.lookup_alias(identity, Some(context_id)).await?;

                environment.output.write(&res);
            }

            ContextIdentityAliasSubcommand::List { context: _ } => {
                let res = client.list_aliases::<PublicKey>(Some(context_id)).await?;

                environment.output.write(&res);
            }
        }

        Ok(())
    }
}
