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
use crate::common::{
    fetch_multiaddr, load_config, make_request, multiaddr_to_url, resolve_alias, RequestType,
};
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
        let config = load_config(&environment.args.home, &environment.args.node_name).await?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve context")?;

        let revokee_id = resolve_alias(multiaddr, &config.identity, self.revokee, Some(context_id))
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve grantee identity")?;

        let endpoint = format!("admin-api/dev/contexts/{}/capabilities/revoke", context_id);
        let url = multiaddr_to_url(multiaddr, &endpoint)?;

        let request: Vec<(PublicKey, ConfigCapability)> =
            vec![(revokee_id, self.capability.into())];

        make_request::<_, RevokePermissionResponse>(
            environment,
            &client,
            url,
            Some(request),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        Ok(())
    }
}
