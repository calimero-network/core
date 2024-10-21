use calimero_server_primitives::admin::GetContextsResponse;
use clap::Parser;
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{
    fetch_multiaddr, get_response, load_config, multiaddr_to_url, CliError, RequestType,
};

#[derive(Debug, Parser)]
pub struct ListCommand {}

impl ListCommand {
    pub async fn run(self, args: RootArgs) -> Result<GetContextsResponse, CliError> {
        let config = load_config(&args.node_name)?;

        let response = get_response(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/contexts")?,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        if !response.status().is_success() {
            return Err(CliError::MethodCallError(format!(
                "List contexts request failed with status: {}",
                response.status()
            )));
        }

        let body: GetContextsResponse = response
            .json()
            .await
            .map_err(|e| CliError::MethodCallError(e.to_string()))?;

        Ok(body)
    }
}
