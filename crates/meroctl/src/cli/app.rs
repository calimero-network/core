use clap::{Parser, Subcommand};
use eyre::Result as EyreResult;

use super::RootArgs;
use crate::cli::app::get::GetCommand;
use crate::cli::app::install::InstallCommand;
use crate::cli::app::list::ListCommand;

mod get;
mod install;
mod list;

#[derive(Debug, Parser)]
pub struct AppCommand {
    #[command(subcommand)]
    pub subcommand: AppSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum AppSubCommands {
    Install(InstallCommand),
    #[command(alias = "ls")]
    List(ListCommand),
    Get(GetCommand),
}

impl AppCommand {
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        match self.subcommand {
            AppSubCommands::Install(install) => install.run(args).await,
            AppSubCommands::List(list) => list.run(args).await,
            AppSubCommands::Get(get) => get.run(args).await,
        }
    }
}
