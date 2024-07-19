use chrono::Utc;
use clap::Parser;
use reqwest::Client;

use crate::cli::context::common::multiaddr_to_url;
use crate::cli::RootArgs;
use crate::config_file::ConfigFile;

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
        let keypair = config.identity;

        let timestamp = Utc::now().timestamp().to_string();
        let signature = keypair.sign(timestamp.as_bytes())?;

        let response = client
            .get(url)
            .header("X-Signature", hex::encode(signature))
            .header("X-Timestamp", timestamp)
            .send()
            .await?;

        if response.status().is_success() {
            let api_response: calimero_server_primitives::admin::GetContextsResponse =
                response.json().await?;
            let contexts = api_response.data.contexts;
            for context in contexts {
                println!("{}", context.id);
            }
        } else {
            eyre::bail!("Request failed with status: {}", response.status());
        }

        Ok(())
    }
}
