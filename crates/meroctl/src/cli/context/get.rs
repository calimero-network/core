use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    GetContextClientKeysResponse, GetContextIdentitiesResponse, GetContextResponse,
    GetContextStorageResponse, GetContextUsersResponse,
};
use clap::Parser;
use eyre::{OptionExt, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    fetch_multiaddr, load_config, make_request, multiaddr_to_url, resolve_alias, RequestType,
};
use crate::output::Report;

#[derive(Parser, Debug)]
#[command(about = "Fetch details about the context")]
pub struct GetCommand {
    #[command(subcommand)]
    pub command: GetSubcommand,

    #[arg(value_name = "CONTEXT", help = "Context we're operating on")]
    pub context: Alias<ContextId>,
}

#[derive(Debug, Parser)]
pub enum GetSubcommand {
    #[command(about = "Get context information")]
    Info,

    #[command(about = "Get client keys")]
    ClientKeys,

    #[command(about = "Get storage information")]
    Storage,
}

impl Report for GetContextResponse {
    fn report(&self) {
        self.data.report();
    }
}

impl Report for GetContextUsersResponse {
    fn report(&self) {
        for user in &self.data.context_users {
            println!("user_id: {}", user.user_id);
            println!("joined_at: {}", user.joined_at);
        }
    }
}

impl Report for GetContextClientKeysResponse {
    fn report(&self) {
        println!("Client Keys: {self:?}");
    }
}

impl Report for GetContextStorageResponse {
    fn report(&self) {
        println!("Storage: {self:?}");
    }
}

impl Report for GetContextIdentitiesResponse {
    fn report(&self) {
        for identity in &self.data.identities {
            println!("{}", identity);
        }
    }
}

impl GetCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        match self.command {
            GetSubcommand::Info => {
                self.get_context(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                    &context_id,
                )
                .await
            }
            GetSubcommand::ClientKeys => {
                self.get_client_keys(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                    &context_id,
                )
                .await
            }
            GetSubcommand::Storage => {
                self.get_storage(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                    &context_id,
                )
                .await
            }
        }
    }

    async fn get_context(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        context_id: &ContextId,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(multiaddr, &format!("admin-api/dev/contexts/{}", context_id))?;
        make_request::<_, GetContextResponse>(
            environment,
            client,
            url,
            None::<()>,
            keypair,
            RequestType::Get,
        )
        .await
    }

    async fn get_client_keys(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        context_id: &ContextId,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/client-keys", context_id),
        )?;
        make_request::<_, GetContextClientKeysResponse>(
            environment,
            client,
            url,
            None::<()>,
            keypair,
            RequestType::Get,
        )
        .await
    }

    async fn get_storage(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        context_id: &ContextId,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/storage", context_id),
        )?;
        make_request::<_, GetContextStorageResponse>(
            environment,
            client,
            url,
            None::<()>,
            keypair,
            RequestType::Get,
        )
        .await
    }
}
