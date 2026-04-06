use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod alias;
pub mod create;
pub mod delete;
pub mod get;
pub mod identity;
pub mod invite_specialized_node;
pub mod list;
pub mod sync;
pub mod update;
pub mod watch;

pub const EXAMPLES: &str = r"
  # List all contexts
  $ meroctl --node node1 context ls

  # Create a new context
  $ meroctl --node node1 context create --protocol <protocol_id> --application-id <app_id>

  # Create a new context in dev mode
  $ meroctl --node node1 context create --protocol <protocol_id> --watch <path> 

  # Grant permission to manage applications
  $ meroctl --node node1 context identity grant bob ManageApplication --context <context_id> --as alice

  # Revoke permission to manage members
  $ meroctl --node node1 context identity revoke bob ManageMembers --context <context_id> --as alice
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
    List(list::ListCommand),
    Create(Box<create::CreateCommand>),
    InviteSpecializedNode(invite_specialized_node::InviteSpecializedNodeCommand),
    Get(get::GetCommand),
    #[command(alias = "del")]
    Delete(delete::DeleteCommand),
    #[command(alias = "ws")]
    Watch(watch::WatchCommand),
    Update(update::UpdateCommand),
    Identity(identity::ContextIdentityCommand),
    Alias(alias::ContextAliasCommand),
    Use(alias::UseCommand),
    Sync(sync::SyncCommand),
}

impl ContextCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            ContextSubCommands::Create(create) => create.run(environment).await,
            ContextSubCommands::Delete(delete) => delete.run(environment).await,
            ContextSubCommands::Get(get) => get.run(environment).await,
            ContextSubCommands::InviteSpecializedNode(cmd) => cmd.run(environment).await,
            ContextSubCommands::List(list) => list.run(environment).await,
            ContextSubCommands::Watch(watch) => watch.run(environment).await,
            ContextSubCommands::Update(update) => update.run(environment).await,
            ContextSubCommands::Identity(identity) => identity.run(environment).await,
            ContextSubCommands::Alias(alias) => alias.run(environment).await,
            ContextSubCommands::Use(use_cmd) => use_cmd.run(environment).await,
            ContextSubCommands::Sync(sync) => sync.run(environment).await,
        }
    }
}
