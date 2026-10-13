use calimero_server_primitives::admin::FollowNamespaceApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Follow a namespace this node's account was added to, without an invitation")]
pub struct FollowCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(
        long = "peer",
        value_name = "MULTIADDR",
        help = "A multiaddr ending in /p2p/<peer id> of a node holding the namespace \
                (repeatable; another node's `meroctl account show` lists its own)"
    )]
    pub peers: Vec<String>,

    #[clap(
        long,
        value_name = "APPLICATION_ID",
        help = "The hex-encoded ID of the application the namespace runs"
    )]
    pub application_id: String,

    #[clap(
        long,
        value_name = "BYTECODE_ID",
        help = "The hex-encoded bytecode blob ID, when the node must fetch it"
    )]
    pub bytecode_id: Option<String>,
}

impl FollowCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = FollowNamespaceApiRequest {
            peer_addrs: self.peers,
            application_id: self.application_id,
            bytecode_id: self.bytecode_id,
        };

        let client = environment.client()?;
        let response = client.follow_namespace(&self.namespace_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
