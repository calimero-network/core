use clap::{Parser, Subcommand};
use const_format::concatcp;

use crate::cli::RootArgs;

mod create;
mod join;
mod list;

pub const EXAMPLES: &str = r"
  # List all contexts
  $ meroctl --home data/ --node-name node1 context ls

  # Create a new context
  $ meroctl --home data/ --node-name node1 context create --application-id my-app-id

  # Create a new context in dev mode
  $ meroctl --home data/ --node-name node1 context create --dev --path /path/to/app --version 1.0.0
";

#[derive(Debug, Parser)]
#[command(about = "Manage contexts")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct ContextCommand {
    #[command(subcommand)]
    pub subcommand: ContextSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum ContextSubCommands {
    #[command(alias = "ls")]
    List(list::ListCommand),
    Create(create::CreateCommand),
    Join(join::JoinCommand),
}

impl ContextCommand {
    pub async fn run(self, args: RootArgs) -> eyre::Result<()> {
        match self.subcommand {
            ContextSubCommands::List(list) => list.run(args).await,
            ContextSubCommands::Create(create) => create.run(args).await,
            ContextSubCommands::Join(join) => join.run(args).await,
        }
    }
}
