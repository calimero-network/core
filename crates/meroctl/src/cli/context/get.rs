use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    GetContextClientKeysResponse, GetContextIdentitiesResponse, GetContextResponse,
    GetContextStorageResponse, GetContextUsersResponse,
};
use clap::Parser;
use comfy_table::{Cell, Color, Table};
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

    #[arg(
        value_name = "CONTEXT",
        help = "Context we're operating on",
        default_value = "default"
    )]
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
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Context Users").fg(Color::Blue),
            Cell::new("User ID").fg(Color::Blue),
            Cell::new("Joined At").fg(Color::Blue),
        ]);

        for user in &self.data.context_users {
            let _ = table.add_row(vec![user.user_id.clone(), user.joined_at.to_string()]);
        }
        println!("{table}");
    }
}

impl Report for GetContextClientKeysResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Client Keys").fg(Color::Blue),
            Cell::new("Wallet Type").fg(Color::Blue),
            Cell::new("Signing Key").fg(Color::Blue),
            Cell::new("Created At").fg(Color::Blue),
            Cell::new("Context ID").fg(Color::Blue),
        ]);

        for key in &self.data.client_keys {
            let _ = table.add_row(vec![
                format!("{:?}", key.wallet_type),
                format!("{:?}", key.signing_key),
                key.created_at.to_string(),
                key.context_id
                    .map_or_else(|| "None".to_owned(), |id| id.to_string()),
            ]);
        }
        println!("{table}");
    }
}

impl Report for GetContextStorageResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Context Storage").fg(Color::Blue)]);
        let _ = table.add_row(vec![format!("Size: {} bytes", self.data.size_in_bytes)]);
        println!("{table}");
    }
}

impl Report for GetContextIdentitiesResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Context Identities").fg(Color::Blue)]);

        for identity in &self.data.identities {
            let _ = table.add_row(vec![identity.to_string()]);
        }
        println!("{table}");
    }
}

impl GetCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let resolve_response =
            resolve_alias(multiaddr, &config.identity, self.context, None).await?;

        let context_id = resolve_response
            .value()
            .cloned()
            .ok_or_eyre("Failed to resolve context: no value found")?;

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
