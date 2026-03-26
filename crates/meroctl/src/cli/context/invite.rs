use calimero_context_config::types::SignedOpenInvitation;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::InviteToContextOpenInvitationRequest;
use clap::Parser;
use eyre::{OptionExt, Result};

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Create an open invitation to a context")]
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

    #[clap(
        long = "valid-for",
        value_name = "SECONDS",
        help = "How long (in seconds) the invitation is valid",
        default_value = "3600"
    )]
    pub valid_for: u64,
}

impl InviteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let _ignored = self.invite(environment).await?;
        Ok(())
    }

    pub async fn invite(&self, environment: &mut Environment) -> Result<SignedOpenInvitation> {
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
            valid_for_seconds: self.valid_for,
        };

        let response = client.invite_to_context(request).await?;

        environment.output.write(&response);

        let invitation = response
            .data
            .ok_or_else(|| eyre::eyre!("No invitation found in the response"))?;

        Ok(invitation)
    }
}
