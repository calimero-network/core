use calimero_context_config::types::AdmitterEndpoint;
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

    /// Where a joining NODE can dial an admitter.
    ///
    /// Only worth passing when this node cannot work the address out itself —
    /// it has no entry for that account, or the one it has is stale.
    #[clap(
        long = "admitter-multiaddr",
        value_name = "MULTIADDR",
        help = "libp2p multiaddr (with peer id) for an admitter, for a joiner \
                that runs a node (repeatable)."
    )]
    pub admitter_multiaddrs: Vec<String>,

    /// Where a joining KEYHOLDER can reach an admitter over HTTPS.
    ///
    /// The one that survives restarts, and the one a joiner with no node of its
    /// own can actually use.
    #[clap(
        long = "admitter-url",
        value_name = "URL",
        help = "https:// base URL of an admitter's admin API, for a joiner \
                holding only a key (repeatable)."
    )]
    pub admitter_urls: Vec<String>,
}

impl InviteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        // Passed through as given. The two kinds are not interchangeable — a
        // keyholder has no swarm to dial a multiaddr from — so they stay
        // separate flags rather than one the node has to guess the shape of.
        let admitter_hints: Vec<AdmitterEndpoint> = self
            .admitter_multiaddrs
            .into_iter()
            .map(AdmitterEndpoint::Multiaddr)
            .chain(self.admitter_urls.into_iter().map(AdmitterEndpoint::Url))
            .collect();

        let request = CreateGroupInvitationApiRequest {
            expiration_timestamp: self.expiration_timestamp,
            recursive: Some(self.recursive),
            admitters: self.admitters,
            admitter_hints,
        };

        let client = environment.client()?;
        let response = client
            .create_namespace_invitation(&self.namespace_id, request)
            .await?;

        println!("{}", serde_json::to_string_pretty(&response)?);

        Ok(())
    }
}
