use calimero_context_config::types::Capability as ConfigCapability;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{GrantPermissionRequest, GrantPermissionResponse};
use clap::{Parser, ValueEnum};
use eyre::OptionExt;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    fetch_multiaddr, load_config, make_request, multiaddr_to_url, resolve_alias, RequestType,
};
use crate::output::Report;

#[derive(Debug, Clone, ValueEnum, Copy)]
pub enum Capability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

impl From<Capability> for ConfigCapability {
    fn from(value: Capability) -> Self {
        match value {
            Capability::ManageApplication => ConfigCapability::ManageApplication,
            Capability::ManageMembers => ConfigCapability::ManageMembers,
            Capability::Proxy => ConfigCapability::Proxy,
        }
    }
}

#[derive(Debug, Parser)]
pub struct GrantPermissionCommand {
    #[arg(help = "The context ID")]
    #[arg(long, short, default_value = "default")]
    pub context: Alias<ContextId>,

    #[arg(help = "The granter's public key")]
    #[arg(long = "as", default_value = "default")]
    pub granter: Alias<PublicKey>,

    #[arg(help = "The grantee's public key")]
    pub grantee: PublicKey,

    #[arg(help = "The capability to grant")]
    #[clap(value_enum)]
    pub capability: Capability,
}

impl GrantPermissionCommand {
    pub async fn run(self, environment: &Environment) -> eyre::Result<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve context")?;

        let granter_id = resolve_alias(multiaddr, &config.identity, self.granter, Some(context_id))
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve granter identity")?;

        let request = GrantPermissionRequest {
            context_id,
            granter_id,
            grantee_id: self.grantee,
            capability: self.capability.into(),
        };

        make_request::<_, GrantPermissionResponse>(
            environment,
            &client,
            multiaddr_to_url(multiaddr, "admin-api/dev/contexts/grant-permission")?,
            Some(request),
            &config.identity,
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
