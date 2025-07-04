use std::path::PathBuf;

use clap::{Parser, Subcommand};
use const_format::concatcp;

use crate::{abi, build, new};

pub const EXAMPLES: &str = r"
  # Create a new application
  $ cargo mero new gitter

  # Build app
  $ cargo mero build

  # Generate ABI
  $ cargo mero abi

  # Publish app
  $ cargo mero publish
";

#[derive(Debug, Parser)]
#[command(name = "mero")]
#[command(bin_name = "cargo mero")]
#[command(after_help = concatcp!("Examples: ", EXAMPLES))]
pub struct RootCommand {
    #[command(subcommand)]
    command: SubCommands,
}

#[derive(Debug, Subcommand)]
enum SubCommands {
    New(NewCommand),
    Build,
    Abi,
}

#[derive(Debug, Parser)]
pub struct NewCommand {
    pub name: PathBuf,
}

impl RootCommand {
    pub async fn run(self) -> eyre::Result<()> {
        match self.command {
            SubCommands::Abi => abi::run().await,
            SubCommands::New(args) => new::run(args).await,
            SubCommands::Build => build::run().await,
        }
    }
}
