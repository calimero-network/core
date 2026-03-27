use calimero_context_config::types::SignedOpenInvitation;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::JoinContextRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Join an application context")]
pub struct JoinCommand {
    #[clap(
        value_name = "INVITE_JSON",
        help = "The SignedOpenInvitation JSON string for joining the context"
    )]
    pub invitation_json: String,
    #[clap(
        long = "identity",
        value_name = "IDENTITY",
        help = "The public key of the identity that will join",
        default_value = "default"
    )]
    pub identity: Alias<PublicKey>,
    #[clap(long = "name", help = "The alias for the context")]
    pub context: Option<Alias<ContextId>>,
    #[clap(long = "as", help = "The alias for the invitee")]
    pub identity_alias: Option<Alias<PublicKey>>,
}

impl JoinCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?.clone();

        let invitation: SignedOpenInvitation = serde_json::from_str(&self.invitation_json)?;

        let new_member_public_key = client
            .resolve_alias(self.identity, None)
            .await?
            .value()
            .cloned()
            .ok_or_else(|| eyre::eyre!("unable to resolve identity"))?;

        let request = JoinContextRequest::new(invitation, new_member_public_key);
        let response = client.join_context(request).await?;

        environment.output.write(&response);

        if let Some(ref payload) = response.data {
            if let Some(context) = self.context {
                let res = client
                    .create_alias_generic(context, None, payload.context_id)
                    .await?;
                environment.output.write(&res);
            }
            if let Some(identity_alias) = self.identity_alias {
                let res = client
                    .create_alias_generic(
                        identity_alias,
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
