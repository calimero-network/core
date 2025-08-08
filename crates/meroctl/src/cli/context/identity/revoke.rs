use calimero_context_config::types::Capability as ConfigCapability;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::RevokePermissionResponse;
use clap::Parser;
use eyre::{OptionExt, Result};

use super::Capability;
use crate::cli::Environment;
use crate::common::resolve_alias;
use crate::output::Report;

impl Report for RevokePermissionResponse {
    fn report(&self) {
        println!("Permission revoked successfully");
    }
}

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
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let connection = environment.connection();

        let context_id = resolve_alias(connection, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve context")?;

        let revokee_id = resolve_alias(connection, self.revokee, Some(context_id))
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve grantee identity")?;

        let request: Vec<(PublicKey, ConfigCapability)> =
            vec![(revokee_id, self.capability.into())];

        let response: RevokePermissionResponse = connection
            .post(
                &format!("admin-api/contexts/{}/capabilities/revoke", context_id),
                request,
            )
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
