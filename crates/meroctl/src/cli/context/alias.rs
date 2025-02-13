use calimero_primitives::alias::{Alias, Kind as KindPrimitive};
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    CreateIdentityAliasRequest, CreateIdentityAliasResponse, DeleteIdentityAliasResponse,
    GetIdentityAliasRequest, GetIdentityAliasResponse,
};
use clap::Parser;
use eyre::Result as EyreResult;
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{fetch_multiaddr, load_config, make_request, multiaddr_to_url, RequestType};

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
        alias: Alias,

        #[arg(help = "Context hash")]
        context: ContextId,
    },

    #[command(about = "Remove a context alias", alias = "delete", alias = "rm")]
    Remove {
        #[arg(help = "Alias name")]
        alias: Alias,
    },

    #[command(about = "Get the hash attached to a context alias")]
    Get {
        #[arg(help = "Alias name")]
        alias: Alias,
    },
}

impl ContextAliasCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        match self.command {
            ContextAliasSubcommand::Add { alias, context } => {
                add_context_alias(
                    environment,
                    &multiaddr,
                    &client,
                    &config.identity,
                    alias,
                    context,
                )
                .await
            }
            ContextAliasSubcommand::Remove { alias } => {
                remove_context_alias(environment, &multiaddr, &client, &config.identity, alias)
                    .await
            }
            ContextAliasSubcommand::Get { alias } => {
                get_context_alias(environment, &multiaddr, &client, &config.identity, alias).await
            }
        }
    }
}

async fn add_context_alias(
    environment: &Environment,
    multiaddr: &Multiaddr,
    client: &Client,
    keypair: &Keypair,
    alias: Alias,
    context: ContextId,
) -> EyreResult<()> {
    let url = multiaddr_to_url(multiaddr, "admin-api/dev/add-alias")?;
    let request = CreateIdentityAliasRequest {
        alias,
        context_id: None,
        kind: KindPrimitive::Context,
        hash: context.into(),
    };

    make_request::<CreateIdentityAliasRequest, CreateIdentityAliasResponse>(
        environment,
        client,
        url,
        Some(request),
        keypair,
        RequestType::Post,
    )
    .await
}

async fn remove_context_alias(
    environment: &Environment,
    multiaddr: &Multiaddr,
    client: &Client,
    keypair: &Keypair,
    alias: Alias,
) -> EyreResult<()> {
    let url = multiaddr_to_url(multiaddr, "admin-api/dev/remove-alias")?;
    let request = GetIdentityAliasRequest {
        alias,
        context_id: None,
        kind: KindPrimitive::Context,
    };

    make_request::<GetIdentityAliasRequest, DeleteIdentityAliasResponse>(
        environment,
        client,
        url,
        Some(request),
        keypair,
        RequestType::Delete,
    )
    .await
}

async fn get_context_alias(
    environment: &Environment,
    multiaddr: &Multiaddr,
    client: &Client,
    keypair: &Keypair,
    alias: Alias,
) -> EyreResult<()> {
    let url = multiaddr_to_url(multiaddr, "admin-api/dev/get-alias")?;
    let request = GetIdentityAliasRequest {
        alias,
        context_id: None,
        kind: KindPrimitive::Context,
    };

    make_request::<GetIdentityAliasRequest, GetIdentityAliasResponse>(
        environment,
        client,
        url,
        Some(request),
        keypair,
        RequestType::Get,
    )
    .await
}
