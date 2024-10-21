use calimero_primitives::context::ContextInvitationPayload;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};
use clap::Parser;
use reqwest::Client;

use crate::cli::RootArgs;
use crate::common::{
    fetch_multiaddr, get_response, load_config, multiaddr_to_url, CliError, RequestType,
};

#[derive(Debug, Parser)]
pub struct JoinCommand {
    #[clap(value_name = "PRIVATE_KEY")]
    private_key: PrivateKey,
    #[clap(value_name = "INVITE")]
    invitation_payload: ContextInvitationPayload,
}

impl JoinCommand {
    pub async fn run(self, args: RootArgs) -> Result<JoinContextResponse, CliError> {
        let config = load_config(&args.node_name)?;

        let response = get_response(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/contexts/join")?,
            Some(JoinContextRequest::new(
                self.private_key,
                self.invitation_payload,
            )),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        if !response.status().is_success() {
            return Err(CliError::MethodCallError(format!(
                "Join context request failed with status: {}",
                response.status()
            )));
        }

        let body = response
            .json::<JoinContextResponse>()
            .await
            .map_err(|e| CliError::MethodCallError(e.to_string()))?;

        Ok(body)
    }
}
