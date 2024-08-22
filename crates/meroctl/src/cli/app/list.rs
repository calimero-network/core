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

        let url = multiaddr_to_url(multiaddr, "admin-api/dev/applications")?;
        let client = Client::new();
        let response = client.get(url).send().await?;

        if !response.status().is_success() {
            eyre::bail!("Request failed with status: {}", response.status())
        }

        let api_response: calimero_server_primitives::admin::ListApplicationsResponse =
            response.json().await?;
        let app_list = api_response.data.apps;

        for app in app_list {
            println!("{}", app.id);
        }

        Ok(())
    }
}
