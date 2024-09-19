use clap::Parser;
use eyre::{bail, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{get_response, multiaddr_to_url, RequestType};
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct DeleteCommand {
    #[clap(long, short)]
    pub context_id: String,
}

impl DeleteCommand {
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

        self.delete_context(multiaddr, &client, &config.identity)
            .await
    }

    #[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
    async fn delete_context(
        &self,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}", self.context_id),
        )?;
        let response = get_response(client, url, None::<()>, keypair, RequestType::Delete).await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        let text = response.text().await?;
        println!("Context deleted successfully: {text}");
        Ok(())
    }
}
