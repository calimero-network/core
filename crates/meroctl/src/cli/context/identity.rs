use calimero_context_config::types::Capability as ConfigCapability;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::GetContextIdentitiesResponse;
use clap::{Parser, ValueEnum};
use eyre::{OptionExt, Result as EyreResult, WrapErr};

use crate::cli::Environment;
use crate::common::{create_alias, delete_alias, lookup_alias, resolve_alias};
use crate::connection::ConnectionInfo;
use crate::output::ErrorLine;

pub mod alias;
pub mod generate;
pub mod grant;
pub mod revoke;

#[derive(Debug, Clone, ValueEnum, Copy)]
#[clap(rename_all = "PascalCase")]
pub enum Capability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

impl From<Capability> for ConfigCapability {
    fn from(value: Capability) -> Self {
        match value {
            Capability::ManageApplication => ConfigCapability::ManageApplication,
            Capability::ManageMembers => ConfigCapability::ManageMembers,
            Capability::Proxy => ConfigCapability::Proxy,
        }
    }
}

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Manage context identities")]
pub struct ContextIdentityCommand {
    #[command(subcommand)]
    command: ContextIdentitySubcommand,
}

#[derive(Copy, Clone, Debug, Parser)]
pub enum ContextIdentitySubcommand {
    #[command(about = "List identities in a context", alias = "ls")]
    List {
        #[arg(help = "The context whose identities we're listing")]
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
        #[arg(long, help = "Show only owned identities")]
        owned: bool,
    },
    #[command(about = "Manage identity aliases")]
    Alias(alias::ContextIdentityAliasCommand),
    #[command(about = "Generate a new identity keypair", alias = "new")]
    Generate(generate::GenerateCommand),
    #[command(about = "Set default identity for a context")]
    Use {
        #[arg(help = "The identity to set as default")]
        identity: PublicKey,
        #[arg(help = "The context to set the identity for")]
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
        #[arg(long, short, help = "Force overwrite if alias already exists")]
        force: bool,
    },
    Grant(grant::GrantPermissionCommand),
    Revoke(revoke::RevokePermissionCommand),
}

impl ContextIdentityCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let connection = environment.connection()?;

        match self.command {
            ContextIdentitySubcommand::List { context, owned } => {
                list_identities(environment, connection, Some(context), owned).await
            }
            ContextIdentitySubcommand::Alias(cmd) => cmd.run(environment).await,
            ContextIdentitySubcommand::Generate(cmd) => cmd.run(environment).await,

            ContextIdentitySubcommand::Use {
                identity,
                context,
                force,
            } => {
                let resolve_response = resolve_alias(connection, context, None).await?;

                let context_id = resolve_response
                    .value()
                    .cloned()
                    .ok_or_eyre("Failed to resolve context: no value found")?;
                let default_alias: Alias<PublicKey> =
                    "default".parse().expect("'default' is a valid alias name");

                let lookup_result =
                    lookup_alias(connection, default_alias, Some(context_id)).await?;

                if let Some(existing_identity) = lookup_result.data.value {
                    if existing_identity == identity {
                        environment.output.write(&ErrorLine(&format!(
                            "Default alias already points to '{}'. Use --force to overwrite.",
                            existing_identity
                        )));
                        return Ok(());
                    }

                    if !force {
                        environment.output.write(&ErrorLine(&format!(
                            "Default alias already points to '{}'. Use --force to overwrite.",
                            existing_identity
                        )));
                        return Ok(());
                    }
                    environment.output.write(&ErrorLine(&format!(
                        "Overwriting existing default alias from '{}' to '{}'",
                        existing_identity, identity
                    )));
                    let _ = delete_alias(connection, default_alias, Some(context_id))
                        .await
                        .wrap_err("Failed to delete existing default alias")?;
                }

                let res =
                    create_alias(connection, default_alias, Some(context_id), identity).await?;

                environment.output.write(&res);

                Ok(())
            }
            ContextIdentitySubcommand::Grant(grant) => grant.run(environment).await,
            ContextIdentitySubcommand::Revoke(revoke) => revoke.run(environment).await,
        }
    }
}

async fn list_identities(
    environment: &Environment,
    connection: &ConnectionInfo,
    context: Option<Alias<ContextId>>,
    owned: bool,
) -> EyreResult<()> {
    let resolve_response = resolve_alias(
        connection,
        context.unwrap_or_else(|| "default".parse().expect("valid alias")),
        None,
    )
    .await?;

    let context_id = match resolve_response.value().cloned() {
        Some(id) => id,
        None => {
            let context_display = context
                .as_ref()
                .map(|alias| alias.to_string())
                .unwrap_or_else(|| "default".to_owned());
            eyre::bail!("Error: Unable to resolve context '{}'. Please verify the context ID exists or setup default context.", context_display)
        }
    };

    let endpoint = if owned {
        format!("admin-api/contexts/{}/identities-owned", context_id)
    } else {
        format!("admin-api/contexts/{}/identities", context_id)
    };

    let response: GetContextIdentitiesResponse = connection.get(&endpoint).await?;

    environment.output.write(&response);
    Ok(())
}
