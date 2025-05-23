use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::GetContextResponse;
use clap::Parser;
use eyre::{eyre, OptionExt, Result as EyreResult, WrapErr};
use reqwest::Client;

use crate::cli::{ApiError, ConnectionInfo, Environment};
use crate::common::{
    create_alias, delete_alias, do_request, lookup_alias, resolve_alias, RequestType,
};
use crate::output::{ErrorLine, WarnLine};

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
        #[arg(help = "Name of the alias to look up")]
        alias: Alias<ContextId>,
    },
}

impl ContextAliasCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let connection = environment
            .connection
            .as_ref()
            .ok_or_else(|| eyre!("No connection configured"))?;

        let auth_key = connection
            .auth_key
            .as_ref()
            .ok_or_else(|| eyre!("No authentication key configured"))?;

        match self.command {
            ContextAliasSubcommand::Add {
                alias,
                context_id,
                force,
            } => {
                if !context_exists(connection, &context_id).await? {
                    environment.output.write(&ErrorLine(&format!(
                        "Context with ID '{}' does not exist",
                        context_id
                    )));
                    return Ok(());
                }

                let lookup_result =
                    lookup_alias(&connection.api_url, auth_key, alias, None).await?;
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

                    let _ = delete_alias(&connection.api_url, auth_key, alias, None)
                        .await
                        .wrap_err("Failed to delete existing alias")?;
                }

                let res =
                    create_alias(&connection.api_url, auth_key, alias, None, context_id).await?;
                environment.output.write(&res);
            }

            ContextAliasSubcommand::Remove { alias } => {
                let res = delete_alias(&connection.api_url, auth_key, alias, None).await?;

                environment.output.write(&res);
            }
            ContextAliasSubcommand::Get { alias } => {
                let res = lookup_alias(&connection.api_url, auth_key, alias, None).await?;

                environment.output.write(&res);
            }
        }

        Ok(())
    }
}

#[derive(Debug, Parser)]
#[command(about = "Set the default context")]
pub struct UseCommand {
    /// The context to set as default
    pub context: Alias<ContextId>,

    /// Force overwrite if default alias already exists
    #[arg(long, short)]
    pub force: bool,
}

impl UseCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let connection = environment
            .connection
            .as_ref()
            .ok_or_else(|| eyre!("No connection configured"))?;

        let auth_key = connection
            .auth_key
            .as_ref()
            .ok_or_else(|| eyre!("No authentication key configured"))?;

        let default_alias: Alias<ContextId> = "default"
            .parse()
            .wrap_err("Failed to parse 'default' as a valid alias name")?;

        let resolve_response = resolve_alias(&connection.api_url, auth_key, self.context, None)
            .await
            .wrap_err("Failed to resolve context")?;

        let context_id = resolve_response
            .value()
            .cloned()
            .ok_or_eyre("Failed to resolve context: no value found")?;

        let lookup_result =
            lookup_alias(&connection.api_url, auth_key, default_alias, None).await?;
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
            let _ = delete_alias(&connection.api_url, auth_key, default_alias, None)
                .await
                .wrap_err("Failed to delete existing default alias")?;
        }

        let res = create_alias(
            &connection.api_url,
            auth_key,
            default_alias,
            None,
            context_id,
        )
        .await
        .wrap_err("Failed to set default context")?;

        environment.output.write(&res);

        if self.context.as_str() == context_id.as_str() {
            println!("Default context set to: {}", context_id);
        } else {
            println!(
                "Default context set to: {} (from alias '{}')",
                context_id, self.context
            );
        }

        Ok(())
    }
}

async fn context_exists(connection: &ConnectionInfo, target_id: &ContextId) -> EyreResult<bool> {
    let mut url = connection.api_url.clone();
    url.set_path(&format!("admin-api/dev/contexts/{}", target_id));

    let result = do_request::<_, GetContextResponse>(
        &Client::new(),
        url,
        None::<()>,
        connection.auth_key.as_ref(),
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
