//! CLI command for inviting specialized nodes (e.g., read-only TEE nodes) to a context.

use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::InviteSpecializedNodeRequest;
use clap::Parser;
use eyre::{OptionExt, Result};

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Invite specialized nodes (e.g., read-only TEE nodes) to join a context")]
pub struct InviteSpecializedNodeCommand {
    #[clap(
        long,
        short,
        value_name = "CONTEXT",
        help = "The context to invite specialized nodes to",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,

    #[clap(
        long = "as",
        value_name = "INVITER",
        help = "The identifier of the inviter (defaults to context's default identity)"
    )]
    pub inviter: Option<Alias<PublicKey>>,
}

impl InviteSpecializedNodeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?.clone();

        // Resolve context alias
        let context_id = client
            .resolve_alias(self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve context alias")?;

        // Resolve inviter alias if provided
        let inviter_id = if let Some(inviter_alias) = self.inviter {
            Some(
                client
                    .resolve_alias(inviter_alias, Some(context_id))
                    .await?
                    .value()
                    .cloned()
                    .ok_or_eyre("unable to resolve inviter alias")?,
            )
        } else {
            None
        };

        let request = InviteSpecializedNodeRequest::new(context_id, inviter_id);

        let response = client.invite_specialized_node(request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
