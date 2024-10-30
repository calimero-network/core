use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::CommandContext;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Parser, Debug)]
#[command(about = "Fetch details about the context")]
pub struct GetCommand {
    #[arg(value_name = "METHOD", help = "Method to fetch details", value_enum)]
    pub method: GetRequest,

    #[arg(value_name = "CONTEXT_ID", help = "context_id of the context")]
    pub context_id: String,
}
#[derive(Clone, Debug, ValueEnum)]
pub enum GetRequest {
    Context,
    Users,
    ClientKeys,
    Storage,
    Identities,
}

impl GetCommand {
    pub async fn run(self, context: CommandContext) -> EyreResult<()> {
        let config = load_config(&context.args.home, &context.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        match self.method {
            GetRequest::Context => {
                self.get_context(&multiaddr, &client, &config.identity)
                    .await?;
            }
            GetRequest::Users => {
                self.get_users(&multiaddr, &client, &config.identity)
                    .await?
            }
            GetRequest::ClientKeys => {
                self.get_client_keys(&multiaddr, &client, &config.identity)
                    .await?;
            }
            GetRequest::Storage => {
                self.get_storage(&multiaddr, &client, &config.identity)
                    .await?;
            }
            GetRequest::Identities => {
                self.get_identities(&multiaddr, &client, &config.identity)
                    .await?;
            }
        }

        Ok(())
    }

    async fn get_context(
        &self,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}", self.context_id),
        )?;
        self.make_request(client, url, keypair).await
    }

    async fn get_users(
        &self,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/users", self.context_id),
        )?;
        self.make_request(client, url, keypair).await
    }

    async fn get_client_keys(
        &self,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/client-keys", self.context_id),
        )?;
        self.make_request(client, url, keypair).await
    }

    async fn get_storage(
        &self,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/storage", self.context_id),
        )?;
        self.make_request(client, url, keypair).await
    }

    async fn get_identities(
        &self,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/identities", self.context_id),
        )?;
        self.make_request(client, url, keypair).await
    }

    #[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
    async fn make_request(
        &self,
        client: &Client,
        url: reqwest::Url,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let response = get_response(client, url, None::<()>, keypair, RequestType::Get).await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        let text = response.text().await?;
        println!("{text}");
        Ok(())
    }
}
