use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::RequestType::GET;
use crate::common::{get_response, multiaddr_to_url};
use crate::config_file::ConfigFile;

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

impl GetCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);
        if !ConfigFile::exists(&path) {
            bail!("Config file does not exist")
        }
        let Ok(config) = ConfigFile::load(&path) else {
            bail!("Failed to load config file");
        };
        let Some(multiaddr) = config.network.server.listen.first() else {
            bail!("No address.")
        };

        let client = Client::new();

        match self.method {
            GetRequest::Context => {
                self.get_context(multiaddr, &client, &config.identity)
                    .await?
            }
            GetRequest::Users => self.get_users(multiaddr, &client, &config.identity).await?,
            GetRequest::ClientKeys => {
                self.get_client_keys(multiaddr, &client, &config.identity)
                    .await?
            }
            GetRequest::Storage => {
                self.get_storage(multiaddr, &client, &config.identity)
                    .await?
            }
            GetRequest::Identities => {
                self.get_identities(multiaddr, &client, &config.identity)
                    .await?
            }
        }

        Ok(())
    }

    async fn get_context(
        &self,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &libp2p::identity::Keypair,
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
        keypair: &libp2p::identity::Keypair,
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
        keypair: &libp2p::identity::Keypair,
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
        keypair: &libp2p::identity::Keypair,
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
        keypair: &libp2p::identity::Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/identities", self.context_id),
        )?;
        self.make_request(client, url, keypair).await
    }

    async fn make_request(
        &self,
        client: &Client,
        url: reqwest::Url,
        keypair: &libp2p::identity::Keypair,
    ) -> EyreResult<()> {
        let response = get_response(client, url, None::<()>, keypair, GET).await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        let text = response.text().await?;
        println!("{}", text);
        Ok(())
    }
}
