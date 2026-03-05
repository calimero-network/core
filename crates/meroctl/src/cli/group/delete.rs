use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::DeleteGroupApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Delete a group")]
pub struct DeleteCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        long,
        help = "Public key of the requester (group admin) (defaults to node NEAR identity)"
    )]
    pub requester: Option<PublicKey>,
}

impl DeleteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = DeleteGroupApiRequest {
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.delete_group(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
