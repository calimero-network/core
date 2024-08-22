use clap::Parser;
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::multiaddr_to_url;
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct ListCommand;

impl ListCommand {
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

        let url = multiaddr_to_url(multiaddr, "admin-api/dev/contexts")?;
        let client = Client::new();
        let response = client.get(url).send().await?;

        if !response.status().is_success() {
            eyre::bail!("Request failed with status: {}", response.status())
        }

        let api_response: calimero_server_primitives::admin::GetContextsResponse =
            response.json().await?;
        let contexts = api_response.data.contexts;

        #[allow(clippy::print_stdout)]
        for context in contexts {
            println!("{}", context.id);
        }

        Ok(())
    }
}
