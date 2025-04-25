use calimero_primitives::application::Application;
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::cli::app::get::GetCommand;
use crate::cli::app::install::InstallCommand;
use crate::cli::app::list::ListCommand;
use crate::cli::Environment;
use crate::output::Report;

mod get;
pub mod install;
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

impl Report for Application {
    fn report(&self) {
        println!("application_id: {}", self.id);
        println!("size: {}", self.size);
        println!("blobId: {}", self.blob);
        println!("source: {}", self.source);
        println!("metadata:");
        for item in &self.metadata {
            println!("  {item:?}");
        }
    }

    fn pretty_report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Application Details").fg(Color::Blue)]);
        let _ = table.add_row(vec![format!("ID: {}", self.id)]);
        let _ = table.add_row(vec![format!("Size: {}", self.size)]);
        let _ = table.add_row(vec![format!("Blob ID: {}", self.blob)]);
        let _ = table.add_row(vec![format!("Source: {}", self.source)]);
        let _ = table.add_row(vec!["Metadata:"]);

        for item in &self.metadata {
            let _ = table.add_row(vec![format!("  - {item:?}")]);
        }
        println!("{table}");
    }
}

impl AppCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        match self.subcommand {
            AppSubCommands::Get(get) => get.run(environment).await,
            AppSubCommands::Install(install) => install.run(environment).await,
            AppSubCommands::List(list) => list.run(environment).await,
        }
    }
}
