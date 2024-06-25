use calimero_primitives::identity::Context;
use clap::Parser;
use eyre::eyre;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::get_ip;
use crate::cli::RootArgs;
use crate::config::ConfigFile;
#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextsResponse {
    data: Vec<Context>,
}

#[derive(Debug, Parser)]
pub struct LsCommand {}

impl LsCommand {
    pub async fn run(self, root_args: RootArgs) -> eyre::Result<()> {
        let path = root_args.home.join(&root_args.node_name);
        if ConfigFile::exists(&path) {
            if let Ok(config) = ConfigFile::load(&path) {
                let multiaddr = config
                    .network
                    .server
                    .listen
                    .first()
                    .ok_or_else(|| eyre!("No address."))?;
                let base_url = get_ip(multiaddr)?;
                let url = format!("{}admin-api/contexts-dev", base_url);

                let client = Client::new();
                let response = client.get(&url).send().await?;

                if response.status().is_success() {
                    let api_response: GetContextsResponse = response.json().await?;
                    let contexts = api_response.data;

                    println!("Contexts:");
                    for context in contexts {
                        println!("App ID: {}", context.application_id);
                        println!("Context ID: {}", context.id);
                        println!();
                    }
                } else {
                    return Err(eyre!("Request failed with status: {}", response.status()));
                }
            } else {
                return Err(eyre!("Failed to load config file"));
            }
        } else {
            return Err(eyre!("Config file does not exist"));
        }

        Ok(())
    }
}
