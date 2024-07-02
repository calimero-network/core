use calimero_primitives::identity::Context;
use clap::Parser;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::cli::context::common::multiaddr_to_url;
use crate::cli::RootArgs;
use crate::config_file::ConfigFile;
#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextsResponse {
    data: Vec<Context>,
}

#[derive(Debug, Parser)]
pub struct ListCommand {}

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

        if response.status().is_success() {
            let api_response: GetContextsResponse = response.json().await?;
            let contexts = api_response.data;

            for context in contexts {
                println!("{}", context.id);
            }
        } else {
            eyre::bail!("Request failed with status: {}", response.status());
        }

        Ok(())
    }
}
