use calimero_config::ConfigFile;
use calimero_primitives::alias::{Alias, Kind};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    CreateIdentityAliasRequest, CreateIdentityAliasResponse, DeleteIdentityAliasResponse,
    GetContextIdentitiesResponse, GetIdentityAliasRequest, GetIdentityAliasResponse,
};
use clap::Parser;
use eyre::Result as EyreResult;
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    fetch_multiaddr, load_config, make_request, multiaddr_to_url, resolve_identifier, RequestType,
};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Manage context identities")]
pub struct ContextIdentityCommand {
    #[command(subcommand)]
    command: ContextIdentitySubcommand,
}

#[derive(Debug, Parser)]
pub enum ContextIdentitySubcommand {
    #[command(about = "List identities in a context")]
    List {
        #[arg(long, short, help = "Context ID or alias")]
        context: String,
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
    #[command(about = "Add new alias for an identity in a context", alias = "new")]
    Add {
        #[arg(help = "Alias name")]
        name: Alias,

        #[arg(help = "Identity hash")]
        identity: Hash,

        #[arg(long, short, help = "Context ID or alias")]
        context: String,
    },

    #[command(
        about = "Remove an identity alias from a context",
        alias = "delete",
        alias = "rm"
    )]
    Remove {
        #[arg(help = "Alias name")]
        name: Alias,

        #[arg(long, short, help = "Context ID or alias")]
        context: String,
    },

    #[command(about = "Get the hash attached to an identity alias in a context")]
    Get {
        #[arg(help = "Alias name")]
        name: Alias,

        #[arg(long, short, help = "Context ID or alias")]
        context: String,
    },
}

impl Report for CreateIdentityAliasResponse {
    fn report(&self) {
        println!("Successfully created alias");
    }
}

impl Report for DeleteIdentityAliasResponse {
    fn report(&self) {
        println!("Successfully deleted alias");
    }
}

impl Report for GetIdentityAliasResponse {
    fn report(&self) {
        println!("Identity hash: {}", self.data.hash);
    }
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
                    &context,
                    owned,
                    &config,
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
        let client = Client::new();

        match self.command {
            ContextIdentityAliasSubcommand::Add {
                name,
                identity,
                context,
            } => {
                add_identity_alias(
                    environment,
                    &multiaddr,
                    &client,
                    &config.identity,
                    name,
                    identity,
                    &context,
                    &config,
                )
                .await
            }
            ContextIdentityAliasSubcommand::Remove { name, context } => {
                remove_identity_alias(
                    environment,
                    &multiaddr,
                    &client,
                    &config.identity,
                    name,
                    &context,
                    &config,
                )
                .await
            }
            ContextIdentityAliasSubcommand::Get { name, context } => {
                get_identity_alias(
                    environment,
                    &multiaddr,
                    &client,
                    &config.identity,
                    name,
                    &context,
                    &config,
                )
                .await
            }
        }
    }
}

async fn list_identities(
    environment: &Environment,
    multiaddr: &Multiaddr,
    client: &Client,
    keypair: &Keypair,
    context: &str,
    owned: bool,
    config: &ConfigFile,
) -> EyreResult<()> {
    let context_id: ContextId = resolve_identifier(config, context, Kind::Context, None)
        .await?
        .into();

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

async fn add_identity_alias(
    environment: &Environment,
    multiaddr: &Multiaddr,
    client: &Client,
    keypair: &Keypair,
    alias: Alias,
    identity: Hash,
    context: &str,
    config: &ConfigFile,
) -> EyreResult<()> {
    let context_id = resolve_identifier(config, context, Kind::Context, None)
        .await?
        .into();

    let url = multiaddr_to_url(multiaddr, "admin-api/dev/add-alias")?;
    let request = CreateIdentityAliasRequest {
        alias,
        context_id: Some(context_id),
        kind: Kind::Identity,
        hash: identity,
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

async fn remove_identity_alias(
    environment: &Environment,
    multiaddr: &Multiaddr,
    client: &Client,
    keypair: &Keypair,
    alias: Alias,
    context: &str,
    config: &ConfigFile,
) -> EyreResult<()> {
    let context_id = resolve_identifier(config, context, Kind::Context, None)
        .await?
        .into();

    let url = multiaddr_to_url(multiaddr, "admin-api/dev/remove-alias")?;
    let request = GetIdentityAliasRequest {
        alias,
        context_id: Some(context_id),
        kind: Kind::Identity,
    };

    make_request::<GetIdentityAliasRequest, DeleteIdentityAliasResponse>(
        environment,
        client,
        url,
        Some(request),
        keypair,
        RequestType::Post,
    )
    .await
}

async fn get_identity_alias(
    environment: &Environment,
    multiaddr: &Multiaddr,
    client: &Client,
    keypair: &Keypair,
    alias: Alias,
    context: &str,
    config: &ConfigFile,
) -> EyreResult<()> {
    let context_id = resolve_identifier(config, context, Kind::Context, None)
        .await?
        .into();

    let url = multiaddr_to_url(multiaddr, "admin-api/dev/get-alias")?;
    let request = GetIdentityAliasRequest {
        alias,
        context_id: Some(context_id),
        kind: Kind::Identity,
    };

    make_request::<GetIdentityAliasRequest, GetIdentityAliasResponse>(
        environment,
        client,
        url,
        Some(request),
        keypair,
        RequestType::Post,
    )
    .await
}
