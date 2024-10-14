use std::fmt::Debug;

use calimero_server::admin::handlers::context::{
    GetContextClientKeysResponse, GetContextIdentitiesResponse, GetContextResponse,
    GetContextStorageResponse, GetContextUsersResponse,
};
use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Parser, Debug)]
pub struct GetCommand {
    #[clap(long, short)]
    pub method: GetRequest,

    #[clap(long, short)]
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

enum GetResponse {
    Context(GetContextResponse),
    Users(GetContextUsersResponse),
    ClientKeys(GetContextClientKeysResponse),
    Storage(GetContextStorageResponse),
    Identities(GetContextIdentitiesResponse),
}

impl GetCommand {
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let config = load_config(&args.node_name)?;
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

        let response = match self.method {
            GetRequest::Context => GetResponse::Context(response.json().await?),
            GetRequest::Users => GetResponse::Users(response.json().await?),
            GetRequest::ClientKeys => GetResponse::ClientKeys(response.json().await?),
            GetRequest::Storage => GetResponse::Storage(response.json().await?),
            GetRequest::Identities => GetResponse::Identities(response.json().await?),
        };

        println!("{:#?}", response);
        Ok(())
    }
}

impl Debug for GetResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GetResponse::Context(resp) => write!(f, "{:#?}", resp),
            GetResponse::Users(resp) => write!(f, "{:#?}", resp),
            GetResponse::ClientKeys(resp) => write!(f, "{:#?}", resp),
            GetResponse::Storage(resp) => write!(f, "{:#?}", resp),
            GetResponse::Identities(resp) => write!(f, "{:#?}", resp),
        }
    }
}
