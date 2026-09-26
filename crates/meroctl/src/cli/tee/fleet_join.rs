use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Announce this node as a TEE fleet member for a group")]
pub struct FleetJoinCommand {
    /// Hex-encoded group ID (64 hex chars / 32 bytes).
    #[clap(name = "GROUP_ID")]
    pub group_id: String,

    /// Ask this peer for admission directly, as a libp2p multiaddr ending in
    /// `/p2p/<peer id>`. Repeatable; tried in order, before the broadcast.
    #[clap(long = "admitter-addr", value_name = "MULTIADDR")]
    pub admitter_addrs: Vec<String>,
}

impl FleetJoinCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client
            .fleet_join_with_admitters(self.group_id, self.admitter_addrs)
            .await?;
        environment.output.write(&response);
        Ok(())
    }
}
