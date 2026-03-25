use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::CreateGroupInvitationApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Create a group invitation")]
pub struct InviteCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,

    #[clap(
        long,
        help = "Duration in seconds for the invitation validity (defaults to 1 year)"
    )]
    pub expiration_timestamp: Option<u64>,
}

impl InviteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = CreateGroupInvitationApiRequest {
            requester: self.requester,
            expiration_timestamp: self.expiration_timestamp,
        };

        let client = environment.client()?;
        let response = client
            .create_group_invitation(&self.group_id, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
