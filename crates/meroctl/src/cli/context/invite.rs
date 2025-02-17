use calimero_primitives::alias::Kind;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{InviteToContextRequest, InviteToContextResponse};
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    do_request, fetch_multiaddr, load_config, multiaddr_to_url, resolve_identifier, RequestType,
};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Create invitation to a context for a invitee")]
pub struct InviteCommand {
    #[clap(
        value_name = "CONTEXT_ID",
        help = "The context id or alias for which invitation is created"
    )]
    pub context_id: String,

    #[clap(
        value_name = "INVITER_ID",
        help = "The public key or alias of the inviter"
    )]
    pub inviter_id: String,

    #[clap(
        value_name = "INVITEE_ID",
        help = "The public key or alias of the invitee"
    )]
    pub invitee_id: String,
}

impl Report for InviteToContextResponse {
    fn report(&self) {
        match self.data {
            Some(ref payload) => {
                println!("{:?}", payload)
            }
            None => println!("No invitation payload"),
        }
    }
}

impl InviteCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let _ignored = self.invite(environment).await?;
        Ok(())
    }

    pub async fn invite(&self, environment: &Environment) -> EyreResult<ContextInvitationPayload> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let context_id: ContextId =
            resolve_identifier(&config, &self.context_id, Kind::Context, None)
                .await?
                .into();

        let inviter_id: PublicKey =
            resolve_identifier(&config, &self.inviter_id, Kind::Identity, Some(context_id))
                .await?
                .into();

        let invitee_id: PublicKey =
            resolve_identifier(&config, &self.invitee_id, Kind::Identity, Some(context_id))
                .await?
                .into();

        let response: InviteToContextResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/contexts/invite")?,
            Some(InviteToContextRequest {
                context_id,
                inviter_id,
                invitee_id,
            }),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        environment.output.write(&response);

        let invitation_payload = response
            .data
            .ok_or_else(|| eyre::eyre!("No invitation payload found in the response"))?;

        Ok(invitation_payload)
    }
}
