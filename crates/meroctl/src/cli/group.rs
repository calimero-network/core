use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod contexts;
pub mod delete;
pub mod get;
pub mod join_context;
pub mod leave;
pub mod leave_context;
pub mod members;
pub mod metadata;
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
    Metadata(metadata::MetadataCommand),
    #[command(alias = "member-metadata")]
    MemberMetadata(metadata::MemberMetadataCommand),
    #[command(alias = "context-metadata")]
    ContextMetadata(metadata::ContextMetadataCommand),
    Reparent(reparent::ReparentCommand),
    Subgroups(subgroups::SubgroupsCommand),
    #[command(alias = "signing-key")]
    SigningKey(signing_key::SigningKeyCommand),
    Upgrade(upgrade::UpgradeCommand),
    Sync(sync::SyncCommand),
    #[command(alias = "join-context")]
    JoinContext(join_context::JoinContextCommand),
    /// Leave a context locally (no DAG op published).
    #[command(alias = "leave-context")]
    LeaveContext(leave_context::LeaveContextCommand),
    /// Voluntarily leave a group (publishes MemberLeft).
    Leave(leave::LeaveCommand),
    Settings(settings::SettingsCommand),
}

impl GroupCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        crate::cli::dispatch_subcommands!(
            self.subcommand,
            environment,
            GroupSubCommands::Get,
            GroupSubCommands::Delete,
            GroupSubCommands::Update,
            GroupSubCommands::Members,
            GroupSubCommands::Contexts,
            GroupSubCommands::Metadata,
            GroupSubCommands::MemberMetadata,
            GroupSubCommands::ContextMetadata,
            GroupSubCommands::Reparent,
            GroupSubCommands::Subgroups,
            GroupSubCommands::SigningKey,
            GroupSubCommands::Upgrade,
            GroupSubCommands::Sync,
            GroupSubCommands::JoinContext,
            GroupSubCommands::LeaveContext,
            GroupSubCommands::Leave,
            GroupSubCommands::Settings,
        )
    }
}
