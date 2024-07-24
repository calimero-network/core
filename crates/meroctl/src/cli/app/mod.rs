use clap::{Parser, Subcommand};

use super::RootArgs;

mod install;
mod list;

#[derive(Parser, Debug)]
pub struct AppCommand {
    #[command(subcommand)]
    pub subcommand: AppSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum AppSubCommands {
    Install(install::InstallCommand),
    #[command(alias = "ls")]
    List(list::ListCommand),
}

impl AppCommand {
    pub async fn run(self, args: RootArgs) -> eyre::Result<()> {
        match self.subcommand {
            AppSubCommands::Install(install) => install.run(args).await,
            AppSubCommands::List(list) => list.run(args).await,
        }
    }
}
