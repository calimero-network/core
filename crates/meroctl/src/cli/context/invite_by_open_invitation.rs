use calimero_context_config::types::{BlockHeight, SignedOpenInvitation};
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::InviteToContextOpenInvitationRequest;
use clap::Parser;
use eyre::{OptionExt, Result};

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Create an open invitation to a context")]
pub struct InviteByOpenInvitationCommand {
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

    #[clap(
        long = "valid-for-blocks",
        value_name = "VALID_FOR_BLOCKS",
        help = "The number of blocks for which the invitation is valid",
        default_value = "1000"
    )]
    pub valid_for_blocks: BlockHeight,
}

impl InviteByOpenInvitationCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let _ignored = self.invite_by_open_invitation(environment).await?;
        Ok(())
    }

    pub async fn invite_by_open_invitation(
        &self,
        environment: &mut Environment,
    ) -> Result<SignedOpenInvitation> {
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

        let request = InviteToContextOpenInvitationRequest {
            context_id,
            inviter_id,
            valid_for_blocks: self.valid_for_blocks,
        };

        let response = client.invite_to_context_by_open_invitation(request).await?;

        //if let Some(ref signed_open_invitation) = response.data {
        //        environment.output.write(&res);
        //    }
        //}
        environment.output.write(&response);

        let signed_open_invitation_payload = response.data.ok_or_else(|| {
            eyre::eyre!("No signed open invitation payload found in the response")
        })?;

        Ok(signed_open_invitation_payload)
    }
}
