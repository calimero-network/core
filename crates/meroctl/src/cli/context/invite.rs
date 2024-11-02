use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{InviteToContextRequest, InviteToContextResponse};
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Create invitation to a context for a invitee")]
pub struct InviteCommand {
    #[clap(
        value_name = "CONTEXT_ID",
        help = "The id of the context for which invitation is created"
    )]
    pub context_id: ContextId,

    #[clap(value_name = "INVITER_ID", help = "The public key of the inviter")]
    pub inviter_id: PublicKey,

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
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let response: InviteToContextResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/contexts/invite")?,
            Some(InviteToContextRequest {
                context_id: self.context_id,
                inviter_id: self.inviter_id,
                invitee_id: self.invitee_id,
            }),
            &config.identity,
            RequestType::Post,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
