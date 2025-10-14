use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::JoinContextByOpenInvitationRequest;
use clap::Parser;
use eyre::{Result, WrapErr};

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Join an application context by an open invitation")]
pub struct JoinByOpenInvitationCommand {
    #[clap(
        value_name = "OPEN_INVITE",
        help = "The open invitation payload for joining the context (hex-encoding of borsh)"
    )]
    pub invitation: String, // this is hex-encoded borsh-serialized `SignedOpenInvitation`,
    #[clap(long = "name", help = "The alias for the context")]
    pub context: Option<Alias<ContextId>>,
    #[clap(
        long = "as",
        help = "The identity of the member who wants to join the context"
    )]
    pub identity: PublicKey,
}

impl JoinByOpenInvitationCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?.clone();

        let invitation = borsh::from_slice(
            &hex::decode(&self.invitation).context("Failed to hex-decode open invitation")?,
        )
        .context("Failed to deserialize open invitation")?;
        let request = JoinContextByOpenInvitationRequest::new(invitation, self.identity);
        let response = client.join_context_by_open_invitation(request).await?;

        environment.output.write(&response);

        if let Some(ref payload) = response.data {
            if let Some(context) = self.context {
                let res = client
                    .create_alias_generic(context, None, payload.context_id)
                    .await?;
                environment.output.write(&res);
            }
            // TODO(identity): should we add alias here?
        }

        Ok(())
    }
}
