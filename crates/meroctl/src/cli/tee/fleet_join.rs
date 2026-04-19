use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Announce this node as a TEE fleet member for a group")]
pub struct FleetJoinCommand {
    /// Hex-encoded group ID (64 hex chars / 32 bytes).
    #[clap(name = "GROUP_ID")]
    pub group_id: String,
}

impl FleetJoinCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.fleet_join(self.group_id).await?;
        environment.output.write(&response);
        Ok(())
    }
}
