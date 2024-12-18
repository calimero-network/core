use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;
use start::StartBootstrapCommand;

use super::Environment;

mod start;

pub const EXAMPLES: &str = r"
  # Setup and run 2 nodes with demo app
  $ meroctl -- --node-name node1 bootstrap start --merod-path /path/to/merod

# Setup and run 2 nodes with provided app
  $ meroctl -- --node-name node1 bootstrap start --merod-path /path/to/merod --app-path /path/to/app

";

#[derive(Debug, Parser)]
#[command(about = "Command for starting bootstrap")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct BootstrapCommand {
    #[command(subcommand)]
    pub subcommand: BootstrapSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum BootstrapSubCommands {
    Start(StartBootstrapCommand),
}

impl BootstrapCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        match self.subcommand {
            BootstrapSubCommands::Start(generate) => generate.run(environment).await,
        }
    }
}
