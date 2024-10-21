use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_server::admin::handlers::context::UpdateApplicationIdResponse;
use calimero_server_primitives::admin::UpdateContextApplicationRequest;
use clap::Parser;
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{
    fetch_multiaddr, get_response, load_config, multiaddr_to_url, CliError, RequestType,
};

#[derive(Debug, Parser)]
pub struct UpdateCommand {
    pub context_id: ContextId,
    #[clap(long = "app_id")]
    pub application_id: ApplicationId,
}

impl UpdateCommand {
    pub async fn run(self, args: &RootArgs) -> Result<UpdateApplicationIdResponse, CliError> {
        let config = load_config(&args.node_name)?;

        let url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            &format!("admin-api/dev/contexts/{}/application", self.context_id),
        )?;

        let request = UpdateContextApplicationRequest::new(self.application_id);

        let response = get_response(
            &Client::new(),
            url,
            Some(request),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        if !response.status().is_success() {
            return Err(CliError::MethodCallError(format!(
                "Update request failed with status: {}",
                response.status()
            )));
        }

        let update_response = response
            .json::<UpdateApplicationIdResponse>()
            .await
            .map_err(|e| CliError::MethodCallError(e.to_string()))?;

        Ok(update_response)
    }
}
