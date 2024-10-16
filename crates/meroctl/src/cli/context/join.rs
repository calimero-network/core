use calimero_primitives::context::ContextInvitationPayload;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;
use tracing::info;

use crate::cli::RootArgs;
use crate::common::{fetch_multiaddr, get_response, load_config, multiaddr_to_url, RequestType};

#[derive(Debug, Parser)]
pub struct JoinCommand {
    #[clap(value_name = "PRIVATE_KEY")]
    private_key: PrivateKey,
    #[clap(value_name = "INVITE")]
    invitation_payload: ContextInvitationPayload,
}

impl JoinCommand {
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        let config = load_config(&args.home, &args.node_name)?;

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
