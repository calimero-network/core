use std::fmt::Display;

use calimero_primitives::application::Application;
use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;
use serde::Serialize;

use crate::cli::app::get::GetCommand;
use crate::cli::app::install::InstallCommand;
use crate::cli::app::list::ListCommand;
use crate::cli::CommandContext;

mod get;
mod install;
mod list;

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl -- --node-name node1 application ls

  # Get details of an application
  $ meroctl -- --node-name node1 application get <APP_ID>
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

#[derive(Debug, Serialize)]
pub(crate) struct ApplicationReport(Application);

impl Display for ApplicationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "id: {}", self.0.id)?;
        writeln!(f, "size: {}", self.0.size)?;
        writeln!(f, "blobId: {}", self.0.blob)?;
        writeln!(f, "source: {}", self.0.source)?;
        writeln!(f, "metadata:")?;
        for item in &self.0.metadata {
            writeln!(f, "  {:?}", item)?;
        }
        Ok(())
    }
}

impl AppCommand {
    pub async fn run(self, context: CommandContext) -> EyreResult<()> {
        match self.subcommand {
            AppSubCommands::Get(get) => get.run(context).await,
            AppSubCommands::Install(install) => install.run(context).await,
            AppSubCommands::List(list) => list.run(context).await,
        }
    }
}
