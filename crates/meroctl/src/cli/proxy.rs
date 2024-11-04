use calimero_server::admin::handlers::proposals::Proposal;
use clap::{Parser, Subcommand};
use eyre::Result as EyreResult;

use super::{Environment, RootArgs};
use crate::output::Report;

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

impl Report for Proposal {
    fn report(&self) {
        println!("{}", self.id);
        println!("{:#?}", self.author);
        println!("{}", self.title);
        println!("{}", self.description);
    }
}

impl ProxyCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        match self.subcommand {
            ProxySubCommands::Get(get) => get.run(environment).await,
        }
    }
}
