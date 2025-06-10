use calimero_context_config::types::Capability as ConfigCapability;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::RevokePermissionResponse;
use clap::Parser;
use eyre::{OptionExt, Result as EyreResult};
use reqwest::Client;

use super::Capability;
use crate::cli::Environment;
use crate::common::{make_request, resolve_alias, RequestType};
use crate::output::Report;

impl Report for RevokePermissionResponse {
    fn report(&self) {
        println!("Permission revoked successfully");
    }
}

#[derive(Debug, Parser)]
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
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let connection = environment
            .connection
            .as_ref()
            .ok_or_eyre("No connection configured")?;

        let context_id = resolve_alias(
            &connection.api_url,
            connection.auth_key.as_ref(),
            self.context,
            None,
        )
        .await?
        .value()
        .cloned()
        .ok_or_eyre("unable to resolve context")?;

        let revokee_id = resolve_alias(
            &connection.api_url,
            connection.auth_key.as_ref(),
            self.revokee,
            Some(context_id),
        )
        .await?
        .value()
        .cloned()
        .ok_or_eyre("unable to resolve grantee identity")?;

        let mut url = connection.api_url.clone();

        url.set_path(&format!(
            "admin-api/dev/contexts/{}/capabilities/revoke",
            context_id
        ));

        let request: Vec<(PublicKey, ConfigCapability)> =
            vec![(revokee_id, self.capability.into())];

        make_request::<_, RevokePermissionResponse>(
            environment,
            &Client::new(),
            url,
            Some(request),
            connection.auth_key.as_ref(),
            RequestType::Post,
        )
        .await?;

        Ok(())
    }
}
