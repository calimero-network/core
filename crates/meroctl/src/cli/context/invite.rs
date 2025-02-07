use std::str::FromStr;

use calimero_config::ConfigFile;
use calimero_primitives::alias::{Alias, Kind};
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    GetIdentityAliasRequest, GetIdentityAliasResponse, InviteToContextRequest,
    InviteToContextResponse,
};
use clap::Parser;
use eyre::{eyre, Result as EyreResult};
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Create invitation to a context for a invitee")]
pub struct InviteCommand {
    #[clap(
        value_name = "CONTEXT_ID",
        help = "The context id or alias for which invitation is created"
    )]
    pub context_id: String,

    #[clap(
        value_name = "INVITER_ID",
        help = "The public key or alias of the inviter"
    )]
    pub inviter_id: String,

    #[clap(value_name = "INVITEE_ID", help = "The public key of the invitee")]
    pub invitee_id: PublicKey,
}

impl Report for InviteToContextResponse {
    fn report(&self) {
        match self.data {
            Some(ref payload) => {
                println!("{:?}", payload)
            }
            None => println!("No invitation payload"),
        }
    }
}

impl InviteCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let _ignored = self.invite(environment).await?;
        Ok(())
    }

    async fn resolve_identifier(
        &self,
        config: &ConfigFile,
        input: &str,
        kind: Kind,
        context_id: Option<ContextId>,
    ) -> EyreResult<Hash> {
        let direct_result = match kind {
            Kind::Context => ContextId::from_str(input)
                .map(|context_id| context_id.into())
                .map_err(|_| eyre!("ContextId parsing failed")),
            Kind::Identity => PublicKey::from_str(input)
                .map(|public_key| public_key.into())
                .map_err(|_| eyre!("PublicKey parsing failed")),
            Kind::Application => return Err(eyre!("Application kind not supported")),
        };

        if let Ok(hash) = direct_result {
            return Ok(hash);
        }

        let alias = Alias::from_str(input)?;
        let request = GetIdentityAliasRequest {
            alias,
            context_id,
            kind,
        };

        let response: GetIdentityAliasResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(config)?, "admin-api/dev/get-alias")?,
            Some(request),
            &config.identity,
            RequestType::Get,
        )
        .await?;

        Ok(response.data.hash)
    }

    pub async fn invite(&self, environment: &Environment) -> EyreResult<ContextInvitationPayload> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let context_id: ContextId = self
            .resolve_identifier(&config, &self.context_id, Kind::Context, None)
            .await?
            .into();

        let inviter_id: PublicKey = self
            .resolve_identifier(&config, &self.inviter_id, Kind::Identity, Some(context_id))
            .await?
            .into();

        let response: InviteToContextResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/contexts/invite")?,
            Some(InviteToContextRequest {
                context_id,
                inviter_id,
                invitee_id: self.invitee_id,
            }),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        environment.output.write(&response);

        let invitation_payload = response
            .data
            .ok_or_else(|| eyre::eyre!("No invitation payload found in the response"))?;

        Ok(invitation_payload)
    }
}
