use calimero_primitives::alias::Alias;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{InviteToContextRequest, InviteToContextResponse};
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{OptionExt, Result};

use crate::cli::Environment;

use crate::output::Report;

#[derive(Copy, Clone, Debug, Parser)]
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
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let _ignored = self.invite(environment).await?;
        Ok(())
    }

    pub async fn invite(&self, environment: &mut Environment) -> Result<ContextInvitationPayload> {
        let client = environment.mero_client()?.clone();

        let context_id = client.resolve_alias(self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let inviter_id = client.resolve_alias(self.inviter, Some(context_id))
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let request = InviteToContextRequest {
            context_id,
            inviter_id,
            invitee_id: self.invitee_id,
        };

        let response = client.invite_to_context(request).await?;

        environment.output.write(&response);

        let invitation_payload = response
            .data
            .ok_or_else(|| eyre::eyre!("No invitation payload found in the response"))?;

        // Handle alias creation separately to avoid borrowing conflicts
        if let Some(name) = self.name {
            let res =
                client.create_alias_generic(name, Some(context_id), self.invitee_id).await?;
            environment.output.write(&res);
        }

        Ok(invitation_payload)
    }
}
