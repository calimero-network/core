use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::GetContextIdentitiesResponse;
use clap::Parser;
use eyre::{OptionExt, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    create_alias, delete_alias, fetch_multiaddr, load_config, lookup_alias, make_request,
    multiaddr_to_url, resolve_alias, RequestType,
};

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
        #[arg(help = "The context whose identities we're listin")]
        context: Alias<ContextId>,
        #[arg(long, help = "Show only owned identities")]
        owned: bool,
    },
    #[command(about = "Manage identity aliases")]
    Alias(ContextIdentityAliasCommand),
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

        #[arg(long, short, help = "The context that the identity is a member of")]
        context: Alias<ContextId>,
    },

    #[command(
        about = "Remove an identity alias from a context",
        aliases = ["rm", "del", "delete"]
    )]
    Remove {
        #[arg(help = "Name of the alias to remove")]
        identity: Alias<PublicKey>,

        #[arg(long, short, help = "The context that the identity is a member of")]
        context: Alias<ContextId>,
    },

    #[command(about = "Resolve the alias to a context identity")]
    Get {
        #[arg(help = "Name of the alias to look up")]
        identity: Alias<PublicKey>,

        #[arg(long, short, help = "The context that the identity is a member of")]
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
                    context,
                    owned,
                )
                .await
            }
            ContextIdentitySubcommand::Alias(cmd) => cmd.run(environment).await,
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
            } => {
                let res = resolve_alias(multiaddr, &config.identity, context, None).await?;

                let context_id = res.value().ok_or_eyre("unable to resolve alias")?;

                let res = create_alias(
                    multiaddr,
                    &config.identity,
                    name,
                    Some(*context_id),
                    identity,
                )
                .await?;

                environment.output.write(&res);
            }
            ContextIdentityAliasSubcommand::Remove { identity, context } => {
                let res = resolve_alias(multiaddr, &config.identity, context, None).await?;

                let context_id = res.value().ok_or_eyre("unable to resolve alias")?;

                let res =
                    delete_alias(multiaddr, &config.identity, identity, Some(*context_id)).await?;

                environment.output.write(&res);
            }
            ContextIdentityAliasSubcommand::Get { identity, context } => {
                let res = resolve_alias(multiaddr, &config.identity, context, None).await?;

                let context_id = res.value().ok_or_eyre("unable to resolve alias")?;

                let res =
                    lookup_alias(multiaddr, &config.identity, identity, Some(*context_id)).await?;

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
    context: Alias<ContextId>,
    owned: bool,
) -> EyreResult<()> {
    let context_id = resolve_alias(multiaddr, keypair, context, None)
        .await?
        .value()
        .cloned()
        .ok_or_eyre("unable to resolve alias")?;

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
