use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod fleet_join;

pub const EXAMPLES: &str = r"
  # Announce this node as a TEE fleet member and auto-join all contexts
  # in the group once admission succeeds.
  $ meroctl --node node1 tee fleet-join <GROUP_ID>
";

#[derive(Debug, Parser)]
#[command(about = "TEE fleet-node commands")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct TeeCommand {
    #[command(subcommand)]
    pub subcommand: TeeSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum TeeSubCommands {
    #[command(name = "fleet-join", alias = "fleet_join")]
    FleetJoin(fleet_join::FleetJoinCommand),
}

impl TeeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            TeeSubCommands::FleetJoin(cmd) => cmd.run(environment).await,
        }
    }
}
