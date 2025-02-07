use calimero_primitives::alias::{Alias, Kind};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{
    CreateIdentityAliasRequest, CreateIdentityAliasResponse, DeleteIdentityAliasResponse,
    GenerateContextIdentityResponse, GetIdentityAliasRequest, GetIdentityAliasResponse,
};
use clap::builder::PossibleValue;
use clap::{Parser, ValueEnum};
use eyre::{eyre, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{fetch_multiaddr, load_config, make_request, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Copy, Clone, Debug)]
pub struct CliKind(Kind);

impl ValueEnum for CliKind {
    fn value_variants<'a>() -> &'a [Self] {
        &[
            CliKind(Kind::Context),
            CliKind(Kind::Identity),
            CliKind(Kind::Application),
        ]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        match self.0 {
            Kind::Context => Some(PossibleValue::new("context")),
            Kind::Identity => Some(PossibleValue::new("identity")),
            Kind::Application => Some(PossibleValue::new("application")),
        }
    }
}

impl From<CliKind> for Kind {
    fn from(cli_kind: CliKind) -> Self {
        cli_kind.0
    }
}

impl From<Kind> for CliKind {
    fn from(kind: Kind) -> Self {
        CliKind(kind)
    }
}

#[derive(Debug, Parser)]
#[command(about = "Managing your identity and alias")]
pub struct IdentityCommand {
    #[command(subcommand)]
    command: IdentitySubcommand,
}

#[derive(Debug, Parser)]
pub enum IdentitySubcommand {
    #[command(about = "Create public/private key pair used for context identity")]
    New,

    #[command(about = "Manage identity aliases")]
    Alias(AliasCommand),
}

#[derive(Clone, Debug, Parser)]
pub struct AliasCommand {
    #[command(subcommand)]
    command: AliasSubcommand,
}

#[derive(Clone, Debug, Parser)]
pub enum AliasSubcommand {
    #[command(about = "Add new alias for an identity", alias = "create")]
    Add {
        #[arg(help = "Alias name")]
        alias: Alias,

        #[arg(help = "Identity hash")]
        identity: Hash,

        #[arg(long, short, value_enum, help = "Kind of alias", default_value_t = CliKind(Kind::Identity))]
        kind: CliKind,

        #[arg(
            long,
            short,
            help = "Context id (required only for identity aliases)",
            required_if_eq("kind", "identity")
        )]
        context_id: Option<ContextId>,
    },

    #[command(about = "Remove an alias", alias = "delete", alias = "rm")]
    Remove {
        #[arg(help = "Alias name")]
        alias: Alias,

        #[arg(long, short, value_enum, help = "Kind of alias", default_value_t = CliKind(Kind::Identity))]
        kind: CliKind,

        #[arg(
            long,
            short,
            help = "Context id (required only for identity aliases)",
            required_if_eq("kind", "identity")
        )]
        context_id: Option<ContextId>,
    },

    #[command(about = "Get the hash attached to an alias")]
    Get {
        #[arg(help = "Alias name")]
        alias: Alias,

        #[arg(long, short, value_enum, help = "Kind of alias", default_value_t = CliKind(Kind::Identity))]
        kind: CliKind,

        #[arg(
            long,
            short,
            help = "Context id (required only for identity aliases)",
            required_if_eq("kind", "identity")
        )]
        context_id: Option<ContextId>,
    },
}

impl Report for GenerateContextIdentityResponse {
    fn report(&self) {
        println!("public_key: {}", self.data.public_key);
        println!("private_key: {}", self.data.private_key);
    }
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

fn validate_context_id(kind: &CliKind, context_id: &Option<ContextId>) -> EyreResult<()> {
    match (kind.0, context_id) {
        (Kind::Identity, None) => Err(eyre!("context_id is required for identity aliases")),
        (Kind::Application | Kind::Context, Some(_)) => Err(eyre!(
            "context_id must not be provided for application or context aliases"
        )),
        _ => Ok(()),
    }
}

impl IdentityCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        match self.command {
            IdentitySubcommand::New => self.get_new_identity(environment),
            IdentitySubcommand::Alias(ref alias_command) => match &alias_command.command {
                AliasSubcommand::Add {
                    alias,
                    identity,
                    context_id,
                    kind,
                } => {
                    validate_context_id(kind, context_id)?;
                    self.add_alias(
                        environment,
                        &multiaddr,
                        &client,
                        &config.identity,
                        alias.clone(),
                        identity,
                        context_id,
                        kind.clone(),
                    )
                    .await
                }
                AliasSubcommand::Remove {
                    alias,
                    context_id,
                    kind,
                } => {
                    validate_context_id(kind, context_id)?;
                    self.remove_alias(
                        environment,
                        &multiaddr,
                        &client,
                        &config.identity,
                        alias.clone(),
                        context_id,
                        kind.clone(),
                    )
                    .await
                }
                AliasSubcommand::Get {
                    alias,
                    context_id,
                    kind,
                } => {
                    validate_context_id(kind, context_id)?;
                    self.get_alias(
                        environment,
                        &multiaddr,
                        &client,
                        &config.identity,
                        alias.clone(),
                        context_id,
                        kind.clone(),
                    )
                    .await
                }
            },
        }
    }

    fn get_new_identity(&self, environment: &Environment) -> EyreResult<()> {
        let private_key = PrivateKey::random(&mut rand::thread_rng());
        let response = GenerateContextIdentityResponse::new(private_key.public_key(), private_key);
        environment.output.write(&response);
        Ok(())
    }

    async fn add_alias(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        alias: Alias,
        identity: &Hash,
        context_id: &Option<ContextId>,
        kind: CliKind,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(multiaddr, "admin-api/dev/add-alias")?;
        let request = CreateIdentityAliasRequest {
            alias,
            context_id: *context_id,
            kind: kind.into(),
            hash: *identity,
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

    async fn get_alias(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        alias: Alias,
        context_id: &Option<ContextId>,
        kind: CliKind,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(multiaddr, "admin-api/dev/get-alias")?;
        let request = GetIdentityAliasRequest {
            alias,
            context_id: *context_id,
            kind: kind.into(),
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

    async fn remove_alias(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        alias: Alias,
        context_id: &Option<ContextId>,
        kind: CliKind,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(multiaddr, "admin-api/dev/remove-alias")?;
        let request = GetIdentityAliasRequest {
            alias,
            context_id: *context_id,
            kind: kind.into(),
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
}
