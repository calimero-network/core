use calimero_primitives::alias::Alias;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    create_alias, do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType,
};
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
    #[clap(long = "name", help = "The alias for the context")]
    pub context: Option<Alias<ContextId>>,
    #[clap(long = "as", help = "The alias for the invitee")]
    pub identity: Option<Alias<PublicKey>>,
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
        let multiaddr = fetch_multiaddr(&config)?;

        let response: JoinContextResponse = do_request(
            &Client::new(),
            multiaddr_to_url(multiaddr, "admin-api/dev/contexts/join")?,
            Some(JoinContextRequest::new(
                self.private_key,
                self.invitation_payload,
            )),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        environment.output.write(&response);

        if let Some(ref payload) = response.data {
            if let Some(context) = self.context {
                let context_id = payload.context_id;
                let res =
                    create_alias(multiaddr, &config.identity, context, None, context_id).await?;
                environment.output.write(&res);
            }
            if let Some(identity) = self.identity {
                let context_id = payload.context_id;
                let public_key = payload.member_public_key;

                let res = create_alias(
                    multiaddr,
                    &config.identity,
                    identity,
                    Some(context_id),
                    public_key,
                )
                .await?;
                environment.output.write(&res);
            }
        }

        Ok(())
    }
}
