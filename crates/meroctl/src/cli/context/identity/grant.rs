use calimero_context_config::types::Capability as ConfigCapability;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::GrantPermissionResponse;
use clap::Parser;
use eyre::OptionExt;
use reqwest::Client;

use super::Capability;
use crate::cli::Environment;
use crate::common::{
    fetch_multiaddr, load_config, make_request, multiaddr_to_url, resolve_alias, RequestType,
};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Grant permissions to a member in a context")]
pub struct GrantPermissionCommand {
    #[arg(help = "The context ID")]
    #[arg(long, short, default_value = "default")]
    pub context: Alias<ContextId>,

    #[arg(help = "The granter's public key")]
    #[arg(long = "as", default_value = "default")]
    pub granter: Alias<PublicKey>,

    #[arg(help = "The grantee's public key")]
    pub grantee: Alias<PublicKey>,

    #[arg(help = "The capability to grant")]
    #[clap(value_enum)]
    pub capability: Capability,
}

impl GrantPermissionCommand {
    pub async fn run(self, environment: &Environment) -> eyre::Result<()> {
        let config = load_config(
            &environment.args.home,
            &environment.args.node.as_deref().unwrap_or("default"),
        )
        .await?;
        let multiaddr = fetch_multiaddr(&config)?;
        let base_url = multiaddr_to_url(multiaddr, "")?;

        let client = Client::new();

        let context_id = resolve_alias(&base_url, Some(&config.identity), self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve context")?;
        let grantee_id = resolve_alias(
            &base_url,
            Some(&config.identity),
            self.grantee,
            Some(context_id),
        )
        .await?
        .value()
        .cloned()
        .ok_or_eyre("unable to resolve grantee identity")?;

        let endpoint = format!("admin-api/dev/contexts/{}/capabilities/grant", context_id);
        let url = multiaddr_to_url(multiaddr, &endpoint)?;

        let request: Vec<(PublicKey, ConfigCapability)> =
            vec![(grantee_id, self.capability.into())];

        make_request::<_, GrantPermissionResponse>(
            environment,
            &client,
            url,
            Some(request),
            Some(&config.identity),
            RequestType::Post,
        )
        .await
    }
}

impl Report for GrantPermissionResponse {
    fn report(&self) {
        println!("Permission granted successfully");
    }
}
