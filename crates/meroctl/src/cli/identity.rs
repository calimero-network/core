use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::cli::identity::generate::GenerateCommand;
use crate::cli::Environment;

mod generate;

pub const EXAMPLES: &str = r"
  #
  $ meroctl -- --node-name node1 identity generate
";

#[derive(Debug, Parser)]
#[command(about = "Command for managing applications")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct IdentityCommand {
    #[command(subcommand)]
    pub subcommand: IdentitySubCommands,
}

#[derive(Debug, Subcommand)]
pub enum IdentitySubCommands {
    Generate(GenerateCommand),
}

impl IdentityCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        match self.subcommand {
            IdentitySubCommands::Generate(generate) => generate.run(environment).await,
        }
    }
}
