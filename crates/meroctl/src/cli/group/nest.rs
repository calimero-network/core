use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::NestGroupApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Nest a child group under a parent group")]
pub struct NestCommand {
    #[clap(name = "PARENT_GROUP_ID", help = "The hex-encoded parent group ID")]
    pub parent_group_id: String,

    #[clap(name = "CHILD_GROUP_ID", help = "The hex-encoded child group ID")]
    pub child_group_id: String,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl NestCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = NestGroupApiRequest {
            child_group_id: self.child_group_id,
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.nest_group(&self.parent_group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
