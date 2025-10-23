use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod install;
pub mod list;
pub mod uninstall;
pub mod update;

pub const EXAMPLES: &str = r"
  # List apps from a registry
  $ meroctl --node node1 app registry list --registry dev

  # Install app from registry
  $ meroctl --node node1 app registry install my-app --registry dev --version 1.0.0

  # Update app from registry
  $ meroctl --node node1 app registry update my-app --registry dev

  # Uninstall app from registry
  $ meroctl --node node1 app registry uninstall my-app --registry dev
";

#[derive(Debug, Parser)]
#[command(about = "Registry-based app management")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct AppRegistryCommand {
    #[command(subcommand)]
    pub subcommand: AppRegistrySubCommands,
}

#[derive(Debug, Subcommand)]
pub enum AppRegistrySubCommands {
    #[command(alias = "ls")]
    List(list::ListCommand),
    Install(install::InstallCommand),
    Update(update::UpdateCommand),
    Uninstall(uninstall::UninstallCommand),
}

impl AppRegistryCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            AppRegistrySubCommands::List(list) => list.run(environment).await,
            AppRegistrySubCommands::Install(install) => install.run(environment).await,
            AppRegistrySubCommands::Update(update) => update.run(environment).await,
            AppRegistrySubCommands::Uninstall(uninstall) => uninstall.run(environment).await,
        }
    }
}
