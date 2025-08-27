use calimero_context_config::types::Capability as ConfigCapability;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::Parser;
use eyre::{OptionExt, Result};

use super::Capability;
use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Revoke permissions from a member in a context")]
pub struct RevokePermissionCommand {
    #[clap(long, short, default_value = "default")]
    pub context: Alias<ContextId>,

    #[clap(long = "as", default_value = "default")]
    pub revoker: Alias<PublicKey>,

    #[clap(help = "The member to revoke permissions from")]
    pub revokee: Alias<PublicKey>,

    #[clap(help = "The capability to revoke")]
    #[clap(value_enum)]
    pub capability: Capability,
}

impl RevokePermissionCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let context_id = client
            .resolve_alias(self.context, None)
            .await?
            .value()
            .copied()
            .ok_or_eyre("unable to resolve context")?;

        let revokee_id = client
            .resolve_alias(self.revokee, Some(context_id))
            .await?
            .value()
            .copied()
            .ok_or_eyre("unable to resolve grantee identity")?;

        let request: Vec<(PublicKey, ConfigCapability)> =
            vec![(revokee_id, self.capability.into())];

        let response = client.revoke_permissions(&context_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
