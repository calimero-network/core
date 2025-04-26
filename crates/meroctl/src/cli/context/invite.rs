use calimero_primitives::alias::Alias;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{InviteToContextRequest, InviteToContextResponse};
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{OptionExt, Result as EyreResult};
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    create_alias, do_request, fetch_multiaddr, load_config, multiaddr_to_url, resolve_alias,
    RequestType,
};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Create invitation to a context")]
pub struct InviteCommand {
    #[clap(long, short)]
    #[clap(
        value_name = "CONTEXT",
        help = "The context for which invitation is created",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,

    #[clap(
        long = "as",
        value_name = "INVITER",
        help = "The identifier of the inviter",
        default_value = "default"
    )]
    pub inviter: Alias<PublicKey>,

    #[clap(value_name = "INVITEE", help = "The identifier of the invitee")]
    pub invitee_id: PublicKey,

    #[clap(value_name = "ALIAS", help = "The alias for the invitee")]
    pub name: Option<Alias<PublicKey>>,
}

impl Report for InviteToContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.add_row(vec![
            Cell::new("Invitation Details").fg(Color::Blue),
            Cell::new("").fg(Color::Blue),
        ]);

        match &self.data {
            Some(payload) => {
                let payload_str = payload.to_string();
                let _ = table.add_row(vec!["Encoded Payload", &payload_str]);
                let _ = table.add_row(vec!["Length", &payload_str.len().to_string()]);

                if payload_str.len() > 50 {
                    let _ = table.add_row(vec!["Preview", &format!("{}...", &payload_str[..50])]);
                }
            }
            None => {
                let _ = table.add_row(vec!["Status", "No invitation payload available"]);
            }
        }

        println!("{table}");
    }
}

impl InviteCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let _ignored = self.invite(environment).await?;
        Ok(())
    }

    pub async fn invite(&self, environment: &Environment) -> EyreResult<ContextInvitationPayload> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let multiaddr = fetch_multiaddr(&config)?;

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let inviter_id = resolve_alias(multiaddr, &config.identity, self.inviter, Some(context_id))
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let response: InviteToContextResponse = do_request(
            &Client::new(),
            multiaddr_to_url(multiaddr, "admin-api/dev/contexts/invite")?,
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

        if let Some(name) = self.name {
            let res = create_alias(
                multiaddr,
                &config.identity,
                name,
                Some(context_id),
                self.invitee_id,
            )
            .await?;
            environment.output.write(&res);
        }

        Ok(invitation_payload)
    }
}
