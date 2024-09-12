use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::cli::context::create::CreateCommand;
use crate::cli::context::delete::DeleteCommand;
use crate::cli::context::get::GetCommand;
use crate::cli::context::join::JoinCommand;
use crate::cli::context::list::ListCommand;
use crate::cli::RootArgs;

mod create;
mod delete;
mod get;
mod join;
mod list;

pub const EXAMPLES: &str = r"
  # List all contexts
  $ meroctl -- --home data --node-name node1 context ls

  # Create a new context
  $ meroctl -- --home data --node-name node1 context create --application-id <appId>

  # Create a new context in dev mode
  $ meroctl -- --home data --node-name node1 context create --watch <path> -c <contextId>
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
    List(ListCommand),
    Create(Box<CreateCommand>),
    Join(JoinCommand),
    Get(GetCommand),
    #[command(alias = "del")]
    Delete(DeleteCommand),
}

impl ContextCommand {
    pub async fn run(self, args: RootArgs) -> EyreResult<()> {
        match self.subcommand {
            ContextSubCommands::Create(create) => create.run(args).await,
            ContextSubCommands::Delete(delete) => delete.run(args).await,
            ContextSubCommands::Get(get) => get.run(args).await,
            ContextSubCommands::Join(join) => join.run(args).await,
            ContextSubCommands::List(list) => list.run(args).await,
        }
    }
}
