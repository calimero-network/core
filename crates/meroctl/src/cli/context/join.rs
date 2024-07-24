use clap::Parser;
use reqwest::Client;
use tracing::info;

use crate::cli::RootArgs;
use crate::common::multiaddr_to_url;
use crate::config_file::ConfigFile;

#[derive(Parser, Debug)]
pub struct JoinCommand {
    #[clap(long, short)]
    context_id: String,
}

impl JoinCommand {
    pub async fn run(self, root_args: RootArgs) -> eyre::Result<()> {
        let path = root_args.home.join(&root_args.node_name);
        if !ConfigFile::exists(&path) {
            eyre::bail!("Config file does not exist")
        }
        let Ok(config) = ConfigFile::load(&path) else {
            eyre::bail!("Failed to load config file");
        };
        let Some(multiaddr) = config.network.server.listen.first() else {
            eyre::bail!("No address.")
        };

        let url = multiaddr_to_url(
            multiaddr,
            &format!("/dev/contexts/{}/join", self.context_id),
        )?;
        let client = Client::new();
        let response = client.post(url).send().await?;

        if !response.status().is_success() {
            eyre::bail!("Request failed with status: {}", response.status())
        }

        info!("Context {} sucesfully joined", self.context_id);

        Ok(())
    }
}
