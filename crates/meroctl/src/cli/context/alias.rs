use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::{eyre, OptionExt, Result, WrapErr};

use crate::cli::Environment;
use crate::output::{ErrorLine, WarnLine};

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Manage context aliases")]
pub struct ContextAliasCommand {
    #[command(subcommand)]
    pub command: ContextAliasSubcommand,
}

#[derive(Copy, Clone, Debug, Parser)]
pub enum ContextAliasSubcommand {
    #[command(about = "Add new alias for a context", aliases = ["new", "create"])]
    Add {
        #[arg(help = "Name for the alias")]
        alias: Alias<ContextId>,

        #[arg(help = "The context to create an alias for")]
        context_id: ContextId,

        #[arg(long, short, help = "Force overwrite if alias already exists")]
        force: bool,
    },

    #[command(about = "Remove a context alias", aliases = ["rm", "del", "delete"])]
    Remove {
        #[arg(help = "Name of the alias to remove")]
        alias: Alias<ContextId>,
    },

    #[command(about = "Resolve the alias to a context")]
    Get {
        #[arg(help = "Name of the alias to look up", default_value = "default")]
        alias: Alias<ContextId>,
    },

    #[command(about = "List all context aliases", alias = "ls")]
    List,
}

impl ContextAliasCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.command {
            ContextAliasSubcommand::Add {
                alias,
                context_id,
                force,
            } => {
                let client = environment.mero_client()?.clone();
                if !context_exists(&client, &context_id).await? {
                    environment.output.write(&ErrorLine(&format!(
                        "Context with ID '{}' does not exist",
                        context_id
                    )));
                    return Ok(());
                }

                let lookup_result = client.lookup_alias(alias, None).await?;
                if let Some(existing_context) = lookup_result.data.value {
                    if existing_context == context_id {
                        environment.output.write(&WarnLine(&format!(
                            "Alias '{alias}' already points to '{context_id}'. Doing nothing."
                        )));
                        return Ok(());
                    }

                    if !force {
                        environment.output.write(&ErrorLine(&format!(
                            "Alias '{alias}' already exists and points to '{existing_context}'. Use --force to overwrite."
                        )));
                        return Ok(());
                    }
                    environment.output.write(&WarnLine(&format!(
                        "Overwriting existing alias '{alias}' from '{existing_context}' to '{context_id}'"
                    )));

                    // Drop client reference to avoid double borrow
                    {
                        let _ignored = client
                            .delete_alias(alias, None)
                            .await
                            .wrap_err("Failed to delete existing alias")?;
                    }
                }

                let res = client
                    .create_alias_generic(alias, None, context_id)
                    .await
                    .map_err(|e| eyre!("Failed to create alias: {}", e))?;
                environment.output.write(&res);
            }

            ContextAliasSubcommand::Remove { alias } => {
                let client = environment.mero_client()?.clone();
                let res = client.delete_alias(alias, None).await?;

                environment.output.write(&res);
            }
            ContextAliasSubcommand::Get { alias } => {
                let client = environment.mero_client()?.clone();
                let res = client.lookup_alias(alias, None).await?;

                environment.output.write(&res);
            }
            ContextAliasSubcommand::List => {
                let client = environment.mero_client()?.clone();
                let res = client.list_aliases::<ContextId>(None).await?;

                environment.output.write(&res);
            }
        }

        Ok(())
    }
}

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Set the default context")]
pub struct UseCommand {
    /// The context to set as default
    pub context: Alias<ContextId>,

    /// Force overwrite if default alias already exists
    #[arg(long, short)]
    pub force: bool,
}

impl UseCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.mero_client()?.clone();

        let default_alias: Alias<ContextId> = "default"
            .parse()
            .wrap_err("Failed to parse 'default' as a valid alias name")?;

        let resolve_response = client
            .resolve_alias(self.context, None)
            .await
            .wrap_err("Failed to resolve context")?;

        let context_id = resolve_response
            .value()
            .cloned()
            .ok_or_eyre("Failed to resolve context: no value found")?;

        let lookup_result = client.lookup_alias(default_alias, None).await?;
        if let Some(existing_context) = lookup_result.data.value {
            if existing_context == context_id {
                environment.output.write(&WarnLine(&format!(
                    "Default alias already points to '{context_id}'. Doing nothing."
                )));
                return Ok(());
            }

            if !self.force {
                environment.output.write(&ErrorLine(&format!(
                    "Default alias already points to '{}'. Use --force to overwrite.",
                    existing_context
                )));
                return Ok(());
            }
            environment.output.write(&WarnLine(&format!(
                "Overwriting existing default alias from '{existing_context}' to '{context_id}'"
            )));
            let _ignored = client
                .delete_alias(default_alias, None)
                .await
                .wrap_err("Failed to delete existing default alias")?;
        }

        let res = client
            .create_alias_generic(default_alias, None, context_id)
            .await
            .wrap_err("Failed to set default context")?;

        environment.output.write(&res);

        Ok(())
    }
}

async fn context_exists(client: &crate::client::Client, target_id: &ContextId) -> Result<bool> {
    let result = client.get_context(target_id).await;

    match result {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}
