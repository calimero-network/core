use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::CreateGroupInvitationApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Create an invitation for a namespace")]
pub struct InviteCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(
        long,
        help = "Public key of the requester (namespace admin). Auto-resolved if omitted"
    )]
    pub requester: Option<PublicKey>,

    #[clap(
        long,
        help = "Duration in seconds for invitation validity (defaults to 1 year)"
    )]
    pub expiration_timestamp: Option<u64>,

    #[clap(
        long,
        help = "Generate invitations recursively for namespace child groups"
    )]
    pub recursive: bool,
}

impl InviteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = CreateGroupInvitationApiRequest {
            requester: self.requester,
            expiration_timestamp: self.expiration_timestamp,
            recursive: Some(self.recursive),
        };

        let client = environment.client()?;
        let response = client
            .create_namespace_invitation(&self.namespace_id, request)
            .await?;

        println!("{}", serde_json::to_string_pretty(&response)?);

        Ok(())
    }
}
