use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};
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
    Build(BuildOpts),
}

#[derive(Debug, Parser)]
pub struct NewCommand {
    pub name: PathBuf,
}

#[derive(Debug, Parser)]
pub struct BuildOpts {
    /// Assert that `Cargo.lock` will remain unchanged
    #[clap(long)]
    pub locked: bool,
    /// Build app in `dev` profile, without optimizations
    #[clap(long)]
    pub no_release: bool,
    /// Use verbose output
    #[clap(long, short)]
    pub verbose: bool,
    /// Do not print cargo log messages
    #[clap(long, short)]
    pub quiet: bool,
    /// Space or comma separated list of features to activate
    /// Supports multiple --features flags
    #[clap(long, short = 'F', action = ArgAction::Append)]
    pub features: Vec<String>,
    /// No default features
    #[clap(long)]
    pub no_default_features: bool,
    /// Package to build
    #[clap(long, short)]
    pub package: Option<String>,
}

impl RootCommand {
    pub async fn run(self) -> eyre::Result<()> {
        match self.command {
            MeroCmd::Mero(command) => match command {
                SubCommands::New(args) => new::run(args).await,
                SubCommands::Build(args) => build::run(args).await,
            },
        }
    }
}
