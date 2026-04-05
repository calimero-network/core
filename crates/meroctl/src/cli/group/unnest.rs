use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::UnnestGroupApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Unnest a child group from a parent group")]
pub struct UnnestCommand {
    #[clap(name = "PARENT_GROUP_ID", help = "The hex-encoded parent group ID")]
    pub parent_group_id: String,

    #[clap(name = "CHILD_GROUP_ID", help = "The hex-encoded child group ID")]
    pub child_group_id: String,

    #[clap(
        long,
        help = "Public key of the requester. Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl UnnestCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = UnnestGroupApiRequest {
            child_group_id: self.child_group_id,
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.unnest_group(&self.parent_group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
