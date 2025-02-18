use alias::ContextAliasCommand;
use calimero_primitives::context::Context;
use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::cli::context::create::CreateCommand;
use crate::cli::context::delete::DeleteCommand;
use crate::cli::context::get::GetCommand;
use crate::cli::context::identity::ContextIdentityCommand;
use crate::cli::context::invite::InviteCommand;
use crate::cli::context::join::JoinCommand;
use crate::cli::context::list::ListCommand;
use crate::cli::context::update::UpdateCommand;
use crate::cli::context::watch::WatchCommand;
use crate::cli::Environment;
use crate::output::Report;

mod alias;
pub mod create;
mod delete;
mod get;
mod identity;
pub mod invite;
pub mod join;
mod list;
mod update;
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
    Invite(InviteCommand),
    Get(GetCommand),
    #[command(alias = "del")]
    Delete(DeleteCommand),
    #[command(alias = "ws")]
    Watch(WatchCommand),
    Update(UpdateCommand),
    Identity(ContextIdentityCommand),
    Alias(ContextAliasCommand),
}

impl Report for Context {
    fn report(&self) {
        println!("id: {}", self.id);
        println!("application_id: {}", self.application_id);
        println!("root_hash: {}", self.root_hash);
    }
}

impl ContextCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        match self.subcommand {
            ContextSubCommands::Create(create) => create.run(environment).await,
            ContextSubCommands::Delete(delete) => delete.run(environment).await,
            ContextSubCommands::Get(get) => get.run(environment).await,
            ContextSubCommands::Invite(invite) => invite.run(environment).await,
            ContextSubCommands::Join(join) => join.run(environment).await,
            ContextSubCommands::List(list) => list.run(environment).await,
            ContextSubCommands::Watch(watch) => watch.run(environment).await,
            ContextSubCommands::Update(update) => update.run(environment).await,
            ContextSubCommands::Identity(identity) => identity.run(environment).await,
            ContextSubCommands::Alias(alias) => alias.run(environment).await,
        }
    }
}
