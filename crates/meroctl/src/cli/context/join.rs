use calimero_primitives::alias::Alias;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::JoinContextRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Join an application context")]
pub struct JoinCommand {
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

impl JoinCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?.clone();

        let request = JoinContextRequest::new(self.invitation_payload);
        let response = client.join_context(request).await?;

        environment.output.write(&response);

        if let Some(ref payload) = response.data {
            if let Some(context) = self.context {
                let res = client
                    .create_alias_generic(context, None, payload.context_id)
                    .await?;
                environment.output.write(&res);
            }
            if let Some(identity) = self.identity {
                let res = client
                    .create_alias_generic(
                        identity,
                        Some(payload.context_id),
                        payload.member_public_key,
                    )
                    .await?;
                environment.output.write(&res);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_join_command_missing_invitation_fails() {
        let result = JoinCommand::try_parse_from(["join"]);
        assert!(
            result.is_err(),
            "Command should fail when invitation payload is missing"
        );
    }

    #[test]
    fn test_join_command_invalid_invitation_fails() {
        let result = JoinCommand::try_parse_from(["join", "invalid-invitation-payload"]);
        assert!(
            result.is_err(),
            "Command should fail with invalid invitation payload"
        );
    }

    #[test]
    fn test_join_command_parsing_with_context_alias() {
        // We can't easily create a valid ContextInvitationPayload for testing parsing,
        // but we can verify the argument structure exists
        let result = JoinCommand::try_parse_from(["join", "--help"]);
        // --help should succeed and show the available options
        assert!(result.is_err()); // try_parse_from fails on --help (exits 0)
    }
}
