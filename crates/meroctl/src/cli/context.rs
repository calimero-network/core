use clap::{Parser, Subcommand};
use const_format::concatcp;

use crate::cli::context::create::CreateCommand;
use crate::cli::context::delete::DeleteCommand;
use crate::cli::context::get::GetCommand;
use crate::cli::context::join::JoinCommand;
use crate::cli::context::list::ListCommand;
use crate::cli::context::update::UpdateCommand;
use crate::cli::context::watch::WatchCommand;
use crate::cli::RootArgs;
use crate::common::{ResponseBody, ToResponseBody};

mod create;
mod delete;
mod get;
mod join;
mod list;
mod update;
mod watch;

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
    #[command(alias = "ws")]
    Watch(WatchCommand),
    Update(UpdateCommand),
}

impl ContextCommand {
    pub async fn run(self, args: RootArgs) -> ResponseBody {
        match self.subcommand {
            ContextSubCommands::Create(create) => create.run(args).await.to_res_body(),
            ContextSubCommands::Delete(delete) => delete.run(args).await.to_res_body(),
            ContextSubCommands::Get(get) => get.run(args).await.to_res_body(),
            ContextSubCommands::Join(join) => join.run(args).await.to_res_body(),
            ContextSubCommands::List(list) => list.run(args).await.to_res_body(),
            ContextSubCommands::Watch(watch) => watch.run(args).await.to_res_body(),
            ContextSubCommands::Update(update) => update.run(&args).await.to_res_body(),
        }
    }
}
