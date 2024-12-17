use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;
use start::StartBootstrapCommand;

use crate::cli::Environment;

mod start;

pub const EXAMPLES: &str = r"
  #
  $ meroctl -- --node-name node1 bootstrap start --merod-path /path/to/merod
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
