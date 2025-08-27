use calimero_primitives::alias::Alias;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{JoinContextRequest, JoinContextResponse};
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::Result;

use crate::cli::Environment;
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Join an application context")]
pub struct JoinCommand {
    #[clap(
        value_name = "INVITE",
        help = "The invitation payload for joining the context"
    )]
    pub invitation_payload: ContextInvitationPayload,
    #[clap(long = "name", help = "The alias for the context")]
    pub context: Option<Alias<ContextId>>,
    #[clap(long = "as", help = "The alias for the invitee")]
    pub identity: Option<Alias<PublicKey>>,
}

impl Report for JoinContextResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Join Context Response").fg(Color::Blue)]);

        if let Some(payload) = &self.data {
            let _ = table.add_row(vec![format!("Context ID: {}", payload.context_id)]);
            let _ = table.add_row(vec![format!(
                "Member Public Key: {}",
                payload.member_public_key
            )]);
        } else {
            let _ = table.add_row(vec!["No response data".to_owned()]);
        }
        println!("{table}");
    }
}

impl JoinCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let _connection = environment.connection()?;
        let client = environment.client()?.clone();

        let request = JoinContextRequest::new(self.invitation_payload);
        let response = client.join_context(request).await?;

        environment.output.write(&response);

        if let Some(ref payload) = response.data {
            if let Some(context) = self.context {
                let res = client
                    .create_alias_generic(context, None, payload.context_id)
                    .await?;
                environment.output.write(&res);
            }
            if let Some(identity) = self.identity {
                let res = client
                    .create_alias_generic(
                        identity,
                        Some(payload.context_id),
                        payload.member_public_key,
                    )
                    .await?;
                environment.output.write(&res);
            }
        }

        Ok(())
    }
}
