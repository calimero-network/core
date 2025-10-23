use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod list;
pub mod remove;
pub mod setup;

pub const EXAMPLES: &str = r"
  # Setup a local registry
  $ meroctl --node node1 registry setup local --name dev --port 8082

  # Setup a remote registry
  $ meroctl --node node1 registry setup remote --name production --url https://registry.example.com

  # List all registries
  $ meroctl --node node1 registry list

  # Remove a registry
  $ meroctl --node node1 registry remove dev
";

#[derive(Debug, Parser)]
#[command(about = "Command for managing registries")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct RegistryCommand {
    #[command(subcommand)]
    pub subcommand: RegistrySubCommands,
}

#[derive(Debug, Subcommand)]
pub enum RegistrySubCommands {
    #[command(alias = "ls")]
    List(list::ListCommand),
    Setup(setup::SetupCommand),
    Remove(remove::RemoveCommand),
}

impl RegistryCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            RegistrySubCommands::List(list) => list.run(environment).await,
            RegistrySubCommands::Setup(setup) => setup.run(environment).await,
            RegistrySubCommands::Remove(remove) => remove.run(environment).await,
        }
    }
}
