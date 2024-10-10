use calimero_primitives::context::ContextInvitationPayload;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;
use tracing::info;

use crate::cli::RootArgs;
use crate::common::{get_response, multiaddr_to_url, RequestType};

#[derive(Debug, Parser)]
pub struct JoinCommand {
    #[clap(value_name = "PRIVATE_KEY")]
    private_key: PrivateKey,
    #[clap(value_name = "INVITE")]
    invitation_payload: ContextInvitationPayload,
}

impl JoinCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);
        let config = crate::common::load_config(&path)?;
        let multiaddr = crate::common::load_multiaddr(&config)?;

        let url = multiaddr_to_url(&multiaddr, "admin-api/dev/contexts/join")?;
        let client = Client::new();
        let response = get_response(
            &client,
            url,
            Some(JoinContextRequest::new(
                self.private_key,
                self.invitation_payload,
            )),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        let Some(body) = response.json::<JoinContextResponse>().await?.data else {
            bail!("Unable to join context");
        };

        info!(
            "Context {} sucesfully joined as {}",
            body.context_id, body.member_public_key
        );

        Ok(())
    }
}
