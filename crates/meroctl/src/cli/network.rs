use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod status;

pub const EXAMPLES: &str = r"
  # Dump this node's swarm connectivity state — listen/external addresses,
  # relay reservations, rendezvous registrations, DCUtR upgrades, AutoNAT.
  $ meroctl --node node1 network status

  # Same, as JSON (use the root --output-format flag).
  $ meroctl --output-format json --node node1 network status
";

#[derive(Debug, Parser)]
#[command(about = "Inspect libp2p networking state")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct NetworkCommand {
    #[command(subcommand)]
    pub subcommand: NetworkSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum NetworkSubCommands {
    Status(status::StatusCommand),
}

impl NetworkCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            NetworkSubCommands::Status(cmd) => cmd.run(environment).await,
        }
    }
}
