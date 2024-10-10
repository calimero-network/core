use calimero_server_primitives::admin::GetContextsResponse;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{get_response, multiaddr_to_url, RequestType};

#[derive(Debug, Parser)]
pub struct ListCommand;

impl ListCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);
        let config = crate::common::load_config(&path)?;
        let multiaddr = crate::common::load_multiaddr(&config)?;
        let url = multiaddr_to_url(&multiaddr, "admin-api/dev/contexts")?;
        let client = Client::new();

        let response =
            get_response(&client, url, None::<()>, &config.identity, RequestType::Get).await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        let api_response: GetContextsResponse = response.json().await?;
        let contexts = api_response.data.contexts;

        #[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
        for context in contexts {
            println!("{}", context.id);
        }

        Ok(())
    }
}
