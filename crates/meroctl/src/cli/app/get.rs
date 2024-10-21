use calimero_server_primitives::admin::GetApplicationResponse;
use clap::{Parser, ValueEnum};
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{
    fetch_multiaddr, get_response, load_config, multiaddr_to_url, CliError, RequestType,
};

#[derive(Parser, Debug)]
pub struct GetCommand {
    #[arg(long, short)]
    pub method: GetValues,

    #[arg(long, short)]
    pub app_id: String,
}
#[derive(ValueEnum, Debug, Clone)]
pub enum GetValues {
    Details,
}

impl GetCommand {
    pub async fn run(self, args: &RootArgs) -> Result<GetApplicationResponse, CliError> {
        let config = load_config(&args.home, &args.node_name)?;

        let url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            &format!("admin-api/dev/applications/{}", self.app_id),
        )?;

        let response = get_response(
            &Client::new(),
            url,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        if !response.status().is_success() {
            return Err(CliError::MethodCallError(format!(
                "Get request failed with status: {}",
                response.status()
            )));
        }

        let body = response
            .json::<GetApplicationResponse>()
            .await
            .map_err(|e| CliError::MethodCallError(e.to_string()))?;

        Ok(body)
    }
}
