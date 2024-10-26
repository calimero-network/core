use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use super::RootArgs;
use crate::cli::app::get::GetCommand;
use crate::cli::app::install::InstallCommand;
use crate::cli::app::list::ListCommand;

mod get;
mod install;
mod list;

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl -- --home data --node-name node1 application ls

  # Get details of an application
  $ meroctl app get <APP_ID>
";

#[derive(Debug, Parser)]
#[command(about = "Command for managing applications")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct AppCommand {
    #[command(subcommand)]
    pub subcommand: AppSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum AppSubCommands {
    Get(GetCommand),
    Install(InstallCommand),
    #[command(alias = "ls")]
    List(ListCommand),
}

impl AppCommand {
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        match self.subcommand {
            AppSubCommands::Get(get) => get.run(args).await,
            AppSubCommands::Install(install) => install.run(args).await,
            AppSubCommands::List(list) => list.run(args).await,
        }
    }
}
