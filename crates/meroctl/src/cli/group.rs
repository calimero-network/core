use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod contexts;
pub mod delete;
pub mod get;
pub mod join_context;
pub mod members;
pub mod reparent;
pub mod settings;
pub mod signing_key;
pub mod subgroups;
pub mod sync;
pub mod update;
pub mod upgrade;

pub const EXAMPLES: &str = r"
  # Create a namespace (root group)
  $ meroctl --node node1 namespace create --application-id <app_id>

  # Get group info
  $ meroctl --node node1 group get <group_id>

  # Invite another node to a namespace
  $ meroctl --node node1 namespace invite <namespace_id>

  # Join a namespace using an invitation payload
  $ meroctl --node node2 namespace join <namespace_id> '<payload>'

  # List direct child groups
  $ meroctl --node node1 group subgroups <group_id>

  # Atomically move a group to a new parent (replaces nest+unnest)
  $ meroctl --node node1 group reparent <group_id> <new_parent_id>

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
    Get(get::GetCommand),
    #[command(alias = "del")]
    Delete(delete::DeleteCommand),
    Update(update::UpdateCommand),
    Members(members::MembersCommand),
    Contexts(contexts::ContextsCommand),
    Reparent(reparent::ReparentCommand),
    Subgroups(subgroups::SubgroupsCommand),
    #[command(alias = "signing-key")]
    SigningKey(signing_key::SigningKeyCommand),
    Upgrade(upgrade::UpgradeCommand),
    Sync(sync::SyncCommand),
    #[command(alias = "join-context")]
    JoinContext(join_context::JoinContextCommand),
    Settings(settings::SettingsCommand),
}

impl GroupCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            GroupSubCommands::Get(cmd) => cmd.run(environment).await,
            GroupSubCommands::Delete(cmd) => cmd.run(environment).await,
            GroupSubCommands::Update(cmd) => cmd.run(environment).await,
            GroupSubCommands::Members(cmd) => cmd.run(environment).await,
            GroupSubCommands::Contexts(cmd) => cmd.run(environment).await,
            GroupSubCommands::Reparent(cmd) => cmd.run(environment).await,
            GroupSubCommands::Subgroups(cmd) => cmd.run(environment).await,
            GroupSubCommands::SigningKey(cmd) => cmd.run(environment).await,
            GroupSubCommands::Upgrade(cmd) => cmd.run(environment).await,
            GroupSubCommands::Sync(cmd) => cmd.run(environment).await,
            GroupSubCommands::JoinContext(cmd) => cmd.run(environment).await,
            GroupSubCommands::Settings(cmd) => cmd.run(environment).await,
        }
    }
}
