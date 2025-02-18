use clap::{Parser, Subcommand};
use eyre::Result as EyreResult;

use super::Environment;

mod get;
use get::GetCommand;

#[derive(Debug, Parser)]
#[command(about = "Command for managing proxy contract")]
pub struct ProxyCommand {
    #[command(subcommand)]
    pub subcommand: ProxySubCommands,
}

#[derive(Debug, Subcommand)]
pub enum ProxySubCommands {
    Get(GetCommand),
}

impl ProxyCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        match self.subcommand {
            ProxySubCommands::Get(get) => get.run(environment).await,
        }
    }
}
