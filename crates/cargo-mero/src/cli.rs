use std::path::PathBuf;

use clap::{Parser, Subcommand};
use const_format::concatcp;

use crate::{build, new};

pub const EXAMPLES: &str = r"
  # Create a new application
  $ cargo mero new gitter

  # Build app
  $ cargo mero build

  # build app with additional cargo arguments
  $ cargo mero build --verbose
";

#[derive(Debug, Parser)]
#[command(name = "mero")]
#[command(bin_name = "cargo")]
#[command(after_help = concatcp!("Examples: ", EXAMPLES))]
pub struct RootCommand {
    #[command(subcommand)]
    command: MeroCmd,
}

#[derive(Debug, Parser)]
enum MeroCmd {
    #[command(subcommand)]
    Mero(SubCommands),
}

#[derive(Debug, Subcommand)]
enum SubCommands {
    New(NewCommand),
    Build(BuildCommand),
}

#[derive(Debug, Parser)]
pub struct NewCommand {
    pub name: PathBuf,
}

#[derive(Debug, Parser)]
pub struct BuildCommand {
    pub args: Vec<String>,
}

impl RootCommand {
    pub async fn run(self) -> eyre::Result<()> {
        match self.command {
            MeroCmd::Mero(command) => match command {
                SubCommands::New(args) => new::run(args).await,
                SubCommands::Build(args) => build::run(args.args).await,
            },
        }
    }
}
