use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{InviteToContextRequest, InviteToContextResponse};
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::identity::open_identity;
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Create invitation to a context for a invitee")]
pub struct InviteCommand {
    #[clap(
        value_name = "CONTEXT_ID",
        help = "The id of the context for which invitation is created"
    )]
    pub context_id: ContextId,

    #[clap(value_name = "INVITEE_ID", help = "The public key of the invitee")]
    pub invitee_id: PublicKey,

    #[clap(
        value_name = "INVITER_ID",
        help = "The public key of the inviter",
        conflicts_with = "identity_name"
    )]
    pub inviter_id: Option<PublicKey>,

    #[clap(
        short = 'i',
        long,
        value_name = "IDENTITY_NAME",
        help = "The identity with which you want to send this invite (public key)"
    )]
    pub identity_name: Option<String>,
}

impl Report for InviteToContextResponse {
    fn report(&self) {
        match self.data {
            Some(ref payload) => {
                println!("Invitation payload: {}", payload.to_string())
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

        let my_public_key = match self.inviter_id {
            Some(id) => id,
            None => open_identity(environment, self.identity_name.as_ref().unwrap())?.public_key,
        };

        let response: InviteToContextResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/contexts/invite")?,
            Some(InviteToContextRequest {
                context_id: self.context_id,
                inviter_id: my_public_key,
                invitee_id: self.invitee_id,
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
