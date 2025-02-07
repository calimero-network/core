use calimero_server_primitives::admin::{
    GetContextClientKeysResponse, GetContextIdentitiesResponse, GetContextResponse,
    GetContextStorageResponse, GetContextUsersResponse,
};
use clap::Parser;
use eyre::Result as EyreResult;
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{fetch_multiaddr, load_config, make_request, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Parser, Debug)]
#[command(about = "Fetch details about the context")]
pub struct GetCommand {
    #[command(subcommand)]
    pub command: GetSubcommand,

    #[arg(value_name = "CONTEXT_ID", help = "context_id of the context")]
    pub context_id: String,
}

#[derive(Debug, Parser)]
pub enum GetSubcommand {
    #[command(about = "Get context information")]
    Info,

    #[command(about = "Get client keys")]
    ClientKeys,

    #[command(about = "Get storage information")]
    Storage,

    #[command(about = "Get identities")]
    Identities {
        #[arg(long, help = "Show only owned identities")]
        owned: bool,
    },
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

        match self.command {
            GetSubcommand::Info => {
                self.get_context(environment, multiaddr, &client, &config.identity)
                    .await
            }
            GetSubcommand::ClientKeys => {
                self.get_client_keys(environment, multiaddr, &client, &config.identity)
                    .await
            }
            GetSubcommand::Storage => {
                self.get_storage(environment, multiaddr, &client, &config.identity)
                    .await
            }
            GetSubcommand::Identities { owned } => {
                self.get_identities(environment, multiaddr, &client, &config.identity, owned)
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
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}", self.context_id),
        )?;
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
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/client-keys", self.context_id),
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
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/storage", self.context_id),
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

    async fn get_identities(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        owned: bool,
    ) -> EyreResult<()> {
        let endpoint = if owned {
            format!(
                "admin-api/dev/contexts/{}/identities-owned",
                self.context_id
            )
        } else {
            format!("admin-api/dev/contexts/{}/identities", self.context_id)
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
}
