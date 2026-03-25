use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::SyncGroupApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Sync a group from local state")]
pub struct SyncCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        long,
        help = "Public key of the requester. Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl SyncCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = SyncGroupApiRequest {
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.sync_group(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
