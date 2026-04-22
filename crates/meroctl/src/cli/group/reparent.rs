use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::ReparentGroupApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(
    about = "Atomically move a group to a new parent (replaces the previous nest+unnest pair)"
)]
pub struct ReparentCommand {
    #[clap(
        name = "GROUP_ID",
        help = "The hex-encoded group ID to move (must already have a parent)"
    )]
    pub group_id: String,

    #[clap(name = "NEW_PARENT_ID", help = "The hex-encoded new parent group ID")]
    pub new_parent_id: String,

    #[clap(
        long,
        help = "Public key of the requester (namespace admin). Auto-resolved from node namespace identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl ReparentCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = ReparentGroupApiRequest {
            new_parent_id: self.new_parent_id,
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.reparent_group(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
