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
        help = "Duration in seconds for invitation validity (defaults to 1 year)"
    )]
    pub expiration_timestamp: Option<u64>,

    #[clap(
        long,
        help = "Generate invitations recursively for namespace child groups"
    )]
    pub recursive: bool,

    /// Accounts permitted to admit a claim of this invitation.
    ///
    /// Without any, the joiner announces itself on the namespace topic and any
    /// ready peer may admit it — which means the invitation is published to
    /// every subscriber of that topic. Naming admitters keeps it off the topic:
    /// the joiner presents it to one of them directly.
    #[clap(
        long = "admitter",
        value_name = "ACCOUNT_HEX",
        help = "Account permitted to admit this invitation (repeatable). \
                Without it, admission is by broadcast."
    )]
    pub admitters: Vec<String>,
}

impl InviteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = CreateGroupInvitationApiRequest {
            expiration_timestamp: self.expiration_timestamp,
            recursive: Some(self.recursive),
            admitters: self.admitters,
        };

        let client = environment.client()?;
        let response = client
            .create_namespace_invitation(&self.namespace_id, request)
            .await?;

        println!("{}", serde_json::to_string_pretty(&response)?);

        Ok(())
    }
}
