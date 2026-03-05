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
        help = "Public key of the requester (group admin) (defaults to node NEAR identity)"
    )]
    pub requester: Option<PublicKey>,

    #[clap(long, help = "Optional specific invitee public key")]
    pub invitee_identity: Option<PublicKey>,

    #[clap(long, help = "Optional expiration as Unix timestamp")]
    pub expiration: Option<u64>,
}

impl InviteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = CreateGroupInvitationApiRequest {
            requester: self.requester,
            invitee_identity: self.invitee_identity,
            expiration: self.expiration,
        };

        let client = environment.client()?;
        let response = client
            .create_group_invitation(&self.group_id, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
