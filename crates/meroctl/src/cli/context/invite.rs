use calimero_primitives::alias::Alias;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::InviteToContextRequest;
use clap::Parser;
use eyre::{OptionExt, Result};

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Create invitation to a context")]
pub struct InviteCommand {
    #[clap(long, short)]
    #[clap(
        value_name = "CONTEXT",
        help = "The context for which invitation is created",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,

    #[clap(
        long = "as",
        value_name = "INVITER",
        help = "The identifier of the inviter",
        default_value = "default"
    )]
    pub inviter: Alias<PublicKey>,

    #[clap(value_name = "INVITEE", help = "The identifier of the invitee")]
    pub invitee_id: PublicKey,

    #[clap(value_name = "ALIAS", help = "The alias for the invitee")]
    pub name: Option<Alias<PublicKey>>,
}



impl InviteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let _ignored = self.invite(environment).await?;
        Ok(())
    }

    pub async fn invite(&self, environment: &mut Environment) -> Result<ContextInvitationPayload> {
        let client = environment.client()?.clone();

        let context_id = client
            .resolve_alias(self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let inviter_id = client
            .resolve_alias(self.inviter, Some(context_id))
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let request = InviteToContextRequest {
            context_id,
            inviter_id,
            invitee_id: self.invitee_id,
        };

        let response = client.invite_to_context(request).await?;

        environment.output.write(&response);

        let invitation_payload = response
            .data
            .ok_or_else(|| eyre::eyre!("No invitation payload found in the response"))?;

        // Handle alias creation separately to avoid borrowing conflicts
        if let Some(name) = self.name {
            let res = client
                .create_alias_generic(name, Some(context_id), self.invitee_id)
                .await?;
            environment.output.write(&res);
        }

        Ok(invitation_payload)
    }
}
