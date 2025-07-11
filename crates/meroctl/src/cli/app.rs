use calimero_primitives::application::Application;
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;
use crate::output::Report;

pub mod get;
pub mod install;
pub mod list;

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl --node node1 application ls

  # Get details of an application
  $ meroctl --node node1 application get <app_id>
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
    Get(get::GetCommand),
    Install(install::InstallCommand),
    #[command(alias = "ls")]
    List(list::ListCommand),
}

impl Report for Application {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Field").fg(Color::Blue),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Application ID", &self.id.to_string()]);
        let _ = table.add_row(vec!["Size", &self.size.to_string()]);
        let _ = table.add_row(vec!["Blob ID", &self.blob.bytecode.to_string()]);
        let _ = table.add_row(vec!["Source", &self.source.to_string()]);
        let _ = table.add_row(vec!["Metadata", ""]);

        for item in &self.metadata {
            let _ = table.add_row(vec!["", &format!("- {item:?}")]);
        }
        println!("{table}");
    }
}

impl AppCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        match self.subcommand {
            AppSubCommands::Get(get) => get.run(environment).await,
            AppSubCommands::Install(install) => install.run(environment).await,
            AppSubCommands::List(list) => list.run(environment).await,
        }
    }
}
