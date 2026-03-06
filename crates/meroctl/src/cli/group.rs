use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod contexts;
pub mod create;
pub mod delete;
pub mod get;
pub mod invite;
pub mod join;
pub mod join_group_context;
pub mod list;
pub mod members;
pub mod signing_key;
pub mod sync;
pub mod update;
pub mod upgrade;

pub const EXAMPLES: &str = r"
  # List all groups
  $ meroctl --node node1 group ls

  # Create a new group
  $ meroctl --node node1 group create --app-key <hex_key> --application-id <app_id> --admin-identity <public_key>

  # Get group info
  $ meroctl --node node1 group get <group_id>

  # Create an invitation to join a group
  $ meroctl --node node1 group invite <group_id> --requester <public_key>

  # Join a group using an invitation payload
  $ meroctl --node node1 group join '<payload>' --joiner-identity <public_key>

  # Register a signing key for a group admin
  $ meroctl --node node1 group signing-key register <group_id> <hex_signing_key>

  # List members of a group
  $ meroctl --node node1 group members list <group_id>

  # List contexts in a group
  $ meroctl --node node1 group contexts list <group_id>
";

#[derive(Debug, Parser)]
#[command(about = "Command for managing groups")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct GroupCommand {
    #[command(subcommand)]
    pub subcommand: GroupSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum GroupSubCommands {
    #[command(alias = "ls")]
    List(list::ListCommand),
    Create(create::CreateCommand),
    Get(get::GetCommand),
    #[command(alias = "del")]
    Delete(delete::DeleteCommand),
    Update(update::UpdateCommand),
    Members(members::MembersCommand),
    Contexts(contexts::ContextsCommand),
    Invite(invite::InviteCommand),
    Join(join::JoinCommand),
    #[command(alias = "signing-key")]
    SigningKey(signing_key::SigningKeyCommand),
    Upgrade(upgrade::UpgradeCommand),
    Sync(sync::SyncCommand),
    #[command(alias = "join-group-context")]
    JoinGroupContext(join_group_context::JoinGroupContextCommand),
}

impl GroupCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            GroupSubCommands::List(cmd) => cmd.run(environment).await,
            GroupSubCommands::Create(cmd) => cmd.run(environment).await,
            GroupSubCommands::Get(cmd) => cmd.run(environment).await,
            GroupSubCommands::Delete(cmd) => cmd.run(environment).await,
            GroupSubCommands::Update(cmd) => cmd.run(environment).await,
            GroupSubCommands::Members(cmd) => cmd.run(environment).await,
            GroupSubCommands::Contexts(cmd) => cmd.run(environment).await,
            GroupSubCommands::Invite(cmd) => cmd.run(environment).await,
            GroupSubCommands::Join(cmd) => cmd.run(environment).await,
            GroupSubCommands::SigningKey(cmd) => cmd.run(environment).await,
            GroupSubCommands::Upgrade(cmd) => cmd.run(environment).await,
            GroupSubCommands::Sync(cmd) => cmd.run(environment).await,
            GroupSubCommands::JoinGroupContext(cmd) => cmd.run(environment).await,
        }
    }
}
