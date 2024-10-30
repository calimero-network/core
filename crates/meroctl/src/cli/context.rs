use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::cli::context::create::CreateCommand;
use crate::cli::context::delete::DeleteCommand;
use crate::cli::context::get::GetCommand;
use crate::cli::context::join::JoinCommand;
use crate::cli::context::list::ListCommand;
use crate::cli::context::watch::WatchCommand;
use crate::cli::CommandContext;

mod create;
mod delete;
mod get;
mod join;
mod list;
mod watch;

pub const EXAMPLES: &str = r"
  # List all contexts
  $ meroctl -- --node-name node1 context ls

  # Create a new context
  $ meroctl --  --node-name node1 context create --application-id <appId>

  # Create a new context in dev mode
  $ meroctl --  --node-name node1 context create --watch <path> -c <contextId>
";

#[derive(Debug, Parser)]
#[command(about = "Command for managing contexts")]
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
    #[command(alias = "ws")]
    Watch(WatchCommand),
}

impl ContextCommand {
    pub async fn run(self, context: CommandContext) -> EyreResult<()> {
        match self.subcommand {
            ContextSubCommands::Create(create) => create.run(context).await,
            ContextSubCommands::Delete(delete) => delete.run(context).await,
            ContextSubCommands::Get(get) => get.run(context).await,
            ContextSubCommands::Join(join) => join.run(context).await,
            ContextSubCommands::List(list) => list.run(context).await,
            ContextSubCommands::Watch(watch) => watch.run(context).await,
        }
    }
}
