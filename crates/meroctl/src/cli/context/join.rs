use calimero_primitives::context::ContextInvitationPayload;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Join an application context")]
pub struct JoinCommand {
    #[clap(
        value_name = "PRIVATE_KEY",
        help = "The private key for signing the join context request"
    )]
    pub private_key: PrivateKey,
    #[clap(
        value_name = "INVITE",
        help = "The invitation payload for joining the context"
    )]
    pub invitation_payload: ContextInvitationPayload,
}

impl Report for JoinContextResponse {
    fn report(&self) {
        match self.data {
            Some(ref payload) => {
                println!("context_id {}", payload.context_id);
                println!("member_public_key: {}", payload.member_public_key);
            }
            None => todo!(),
        }
    }
}

impl JoinCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let response: JoinContextResponse = do_request(
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

        environment.output.write(&response);

        Ok(())
    }
}
