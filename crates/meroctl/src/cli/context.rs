use calimero_primitives::context::Context;
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Table};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;
use crate::output::Report;

pub mod alias;
pub mod create;
pub mod delete;
pub mod get;
pub mod identity;
pub mod invite;
pub mod join;
pub mod list;
pub mod proposals;
pub mod update;
pub mod watch;

pub const EXAMPLES: &str = r"
  # List all contexts
  $ meroctl --node node1 context ls

  # Create a new context
  $ meroctl --node node1 context create --application-id <app_id>

  # Create a new context in dev mode
  $ meroctl --node node1 context create --watch <path> -c <context_id>

  # Grant permission to manage applications
  $ meroctl context identity grant bob ManageApplication --as alice

  # Revoke permission to manage members
  $ meroctl context identity revoke bob ManageMembers --as alice
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
    Join(join::JoinCommand),
    Invite(invite::InviteCommand),
    Get(get::GetCommand),
    #[command(alias = "del")]
    Delete(delete::DeleteCommand),
    #[command(alias = "ws")]
    Watch(watch::WatchCommand),
    Update(update::UpdateCommand),
    Identity(identity::ContextIdentityCommand),
    Alias(alias::ContextAliasCommand),
    Use(alias::UseCommand),
    Proposals(proposals::ProposalsCommand),
}

impl Report for Context {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Context Details").fg(comfy_table::Color::Blue)
        ]);
        let _ = table.add_row(vec![
            Cell::new(format!("ID: {}", self.id)).fg(comfy_table::Color::Yellow)
        ]);
        let _ = table.add_row(vec![Cell::new(format!(
            "Application ID: {}",
            self.application_id
        ))]);
        let _ = table.add_row(vec![Cell::new(format!("Root Hash: {}", self.root_hash))]);
        println!("{table}");
    }
}

impl ContextCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
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
            ContextSubCommands::Use(use_cmd) => use_cmd.run(environment).await,
            ContextSubCommands::Proposals(proposals) => proposals.run(environment).await,
        }
    }
}
