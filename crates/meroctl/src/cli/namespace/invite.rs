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
    /// Without any, the node fills the list in from the group's admins and TEE
    /// nodes. Naming them narrows that set.
    #[clap(
        long = "admitter",
        value_name = "ACCOUNT_HEX",
        help = "Account permitted to admit this invitation (repeatable). \
                Without it, the node uses the group's admins and TEE nodes."
    )]
    pub admitters: Vec<String>,

    /// Where a joiner can dial an admitter.
    ///
    /// Only worth passing when this node cannot work the address out itself —
    /// it has no entry for that account, or the one it has is stale.
    #[clap(
        long = "admitter-addr",
        value_name = "MULTIADDR",
        help = "libp2p multiaddr of an admitter, including its /p2p/<peer-id> \
                suffix (repeatable). Usually unnecessary: the node fills these \
                in from addresses it already has."
    )]
    pub admitter_addrs: Vec<String>,
}

impl InviteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = CreateGroupInvitationApiRequest {
            expiration_timestamp: self.expiration_timestamp,
            recursive: Some(self.recursive),
            admitters: self.admitters,
            admitter_addrs: self.admitter_addrs,
        };

        let client = environment.client()?;
        let response = client
            .create_namespace_invitation(&self.namespace_id, request)
            .await?;

        println!("{}", serde_json::to_string_pretty(&response)?);

        Ok(())
    }
}
