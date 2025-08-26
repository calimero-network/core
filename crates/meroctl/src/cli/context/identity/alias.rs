use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::Parser;
use eyre::{OptionExt, Result, WrapErr};

use crate::cli::Environment;
use crate::common::{create_alias, delete_alias, list_aliases, lookup_alias, resolve_alias};
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
        // Clone the environment to avoid borrowing conflicts
        let mut env_clone = environment.clone();
        let mero_client = env_clone.mero_client()?;
        let connection = environment.connection()?;
        
        // Extract context and resolve it to context_id
        let context_id = match &self.command {
            ContextIdentityAliasSubcommand::Add { context, .. } |
            ContextIdentityAliasSubcommand::Remove { context, .. } |
            ContextIdentityAliasSubcommand::Get { context, .. } |
            ContextIdentityAliasSubcommand::List { context } => {
                resolve_alias(connection, *context, None).await?
                    .value()
                    .cloned()
                    .ok_or_eyre("Failed to resolve context: no value found")?
            }
        };

        match self.command {
            ContextIdentityAliasSubcommand::Add {
                name,
                identity,
                context,
                force,
            } => {
                // Check if identity exists in context using MeroClient
                let response = mero_client
                    .get_context_identities(&context_id, false)
                    .await?;
                let identity_exists = response.data.identities.contains(&identity);
                
                if !identity_exists {
                    environment.output.write(&ErrorLine(&format!(
                        "Identity '{}' does not exist in context '{}'",
                        identity, context
                    )));
                    return Ok(());
                }

                let connection = environment.connection()?;
                let lookup_result = lookup_alias(connection, name, Some(context_id)).await?;

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
                    let _ignored = delete_alias(connection, name, Some(context_id))
                        .await
                        .wrap_err("Failed to delete existing alias")?;
                }

                let res = create_alias(connection, name, Some(context_id), identity).await?;

                environment.output.write(&res);
            }
            ContextIdentityAliasSubcommand::Remove { identity, context: _ } => {
                let connection = environment.connection()?;
                let res = delete_alias(connection, identity, Some(context_id)).await?;

                environment.output.write(&res);
            }
            ContextIdentityAliasSubcommand::Get { identity, context: _ } => {
                let connection = environment.connection()?;
                let res = lookup_alias(connection, identity, Some(context_id)).await?;

                environment.output.write(&res);
            }

            ContextIdentityAliasSubcommand::List { context: _ } => {
                let connection = environment.connection()?;
                let res = list_aliases::<PublicKey>(connection, Some(context_id)).await?;

                environment.output.write(&res);
            }
        }

        Ok(())
    }
}
