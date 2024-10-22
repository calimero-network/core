use calimero_server_primitives::admin::ListApplicationsResponse;
use clap::Parser;
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{
    fetch_multiaddr, get_response, load_config, multiaddr_to_url, CliError, RequestType,
};

#[derive(Debug, Parser)]
pub struct ListCommand {}

impl ListCommand {
    pub async fn run(self, args: &RootArgs) -> Result<ListApplicationsResponse, CliError> {
        let config = load_config(&args.home, &args.node_name)?;

        let response = get_response(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/applications")?,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        if !response.status().is_success() {
            return Err(CliError::MethodCallError(format!(
                "List request failed with status: {}",
                response.status()
            )));
        }

        let body: ListApplicationsResponse = response
            .json()
            .await
            .map_err(|e| CliError::MethodCallError(e.to_string()))?;

        Ok(body)
    }
}
