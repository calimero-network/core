use calimero_server::admin::handlers::context::DeleteContextResponse;
use clap::Parser;
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{
    fetch_multiaddr, get_response, load_config, multiaddr_to_url, CliError, RequestType,
};

#[derive(Debug, Parser)]
pub struct DeleteCommand {
    #[clap(long, short)]
    pub context_id: String,
}

impl DeleteCommand {
    pub async fn run(self, args: RootArgs) -> Result<DeleteContextResponse, CliError> {
        let config = load_config(&args.home, &args.node_name)?;

        let url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            &format!("admin-api/dev/contexts/{}", self.context_id),
        )?;
        let response = get_response(
            &Client::new(),
            url,
            None::<()>,
            &config.identity,
            RequestType::Delete,
        )
        .await?;

        if !response.status().is_success() {
            return Err(CliError::MethodCallError(format!(
                "Delete context request failed with status: {}",
                response.status()
            )));
        }

        let response: DeleteContextResponse = response
            .json()
            .await
            .map_err(|e| CliError::MethodCallError(e.to_string()))?;

        Ok(response)
    }
}
