use calimero_server_primitives::admin::GetContextsResponse;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Debug, Parser)]
pub struct ListCommand;

impl ListCommand {
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let config = load_config(&args.home, &args.node_name)?;

        let response = get_response(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/contexts")?,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

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
