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
    create_alias, delete_alias, fetch_multiaddr, load_config, lookup_alias, make_request,
    multiaddr_to_url, resolve_alias, RequestType,
};
use crate::output::ErrorLine;

#[derive(Debug, Parser)]
#[command(about = "Manage context identities")]
pub struct ContextIdentityCommand {
    #[command(subcommand)]
    command: ContextIdentitySubcommand,
}

#[derive(Debug, Parser)]
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
    Alias(ContextIdentityAliasCommand),
    #[command(about = "Set default identity for a context")]
    Use {
        #[arg(help = "The identity to set as default")]
        identity: PublicKey,
        #[arg(help = "The context to set the identity for")]
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
        #[arg(long, short, help = "Force overwrite if default is already set")]
        force: bool,
    },
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

impl ContextIdentityCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        match self.command {
            ContextIdentitySubcommand::List { context, owned } => {
                list_identities(
                    environment,
                    &multiaddr,
                    &client,
                    &config.identity,
                    Some(context),
                    owned,
                )
                .await
            }
            ContextIdentitySubcommand::Alias(cmd) => cmd.run(environment).await,
            ContextIdentitySubcommand::Use {
                identity,
                context,
                force,
            } => {
                let resolve_response =
                    resolve_alias(multiaddr, &config.identity, context, None).await?;

                let context_id = resolve_response
                    .value()
                    .cloned()
                    .ok_or_eyre("Failed to resolve context: no value found")?;
                let default_alias: Alias<PublicKey> =
                    "default".parse().expect("'default' is a valid alias name");

                let lookup_result =
                    lookup_alias(multiaddr, &config.identity, default_alias, Some(context_id))
                        .await?;

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
                        delete_alias(
                            multiaddr,
                            &config.identity,
                            default_alias,
                            Some(context_id),
                        )
                        .await
                        .wrap_err("Failed to delete existing default alias")?;
                    
                }

                let res = create_alias(
                    multiaddr,
                    &config.identity,
                    default_alias,
                    Some(context_id),
                    identity,
                )
                .await?;

                environment.output.write(&res);
                println!(
                    "Default identity set to: {} for context {}",
                    identity, context_id
                );
                Ok(())
            }
        }
    }
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

                if !identity_exists_in_context(
                    &multiaddr,
                    &Client::new(),
                    &config.identity,
                    &context,
                    &identity,
                )
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

                let lookup_result = lookup_alias(
                    multiaddr,
                    &config.identity,
                    name.clone(),
                    Some(context_id.clone()),
                )
                .await?;

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
                        delete_alias(
                            multiaddr,
                            &config.identity,
                            name,
                            Some(context_id),
                        )
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

async fn list_identities(
    environment: &Environment,
    multiaddr: &Multiaddr,
    client: &Client,
    keypair: &Keypair,
    context: Option<Alias<ContextId>>,
    owned: bool,
) -> EyreResult<()> {
    let resolve_response = resolve_alias(
        multiaddr,
        keypair,
        context.unwrap_or_else(|| "default".parse().expect("valid alias")),
        None,
    )
    .await?;

    let context_id = resolve_response
        .value()
        .cloned()
        .ok_or_eyre("Failed to resolve context: no value found")?;

    let endpoint = if owned {
        format!("admin-api/dev/contexts/{}/identities-owned", context_id)
    } else {
        format!("admin-api/dev/contexts/{}/identities", context_id)
    };

    let url = multiaddr_to_url(multiaddr, &endpoint)?;
    make_request::<_, GetContextIdentitiesResponse>(
        environment,
        client,
        url,
        None::<()>,
        keypair,
        RequestType::Get,
    )
    .await
}
async fn identity_exists_in_context(
    multiaddr: &Multiaddr,
    client: &Client,
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

    let response: GetContextIdentitiesResponse = client
        .get(url)
        .send()
        .await?
        .json::<GetContextIdentitiesResponse>()
        .await?;

    Ok(response.data.identities.contains(target_identity))
}
