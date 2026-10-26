use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod create;
pub mod create_group;
pub mod delete;
pub mod get;
pub mod groups;
pub mod invite;
pub mod join;
pub mod leave;
pub mod list;
pub mod outbound;

pub const EXAMPLES: &str = r"
  # Create a namespace (root group)
  $ meroctl --node node1 namespace create --application-id <app_id>

  # List all namespaces
  $ meroctl --node node1 namespace ls

  # Get a namespace and remove it from this node
  $ meroctl --node node1 namespace get <namespace_id>
  $ meroctl --node node1 namespace delete <namespace_id>

  # Invite another node and join namespace
  $ meroctl --node node1 namespace invite <namespace_id>
  $ meroctl --node node2 namespace join <namespace_id> '<invitation_json>'

  # Approve outbound requests for an app while creating its namespace, then
  # list and narrow what the namespace approves
  $ meroctl --node node1 namespace create --application-id <app_id> --approve-outbound manifest
  $ meroctl --node node1 namespace outbound ls <namespace_id>
  $ meroctl --node node1 namespace outbound set <namespace_id> --application-id <app_id> --approve none

  # List groups inside namespace and create a subgroup
  $ meroctl --node node1 namespace groups <namespace_id>
  $ meroctl --node node1 namespace create-group <namespace_id> --alias my-group
";

#[derive(Debug, Parser)]
#[command(about = "Manage namespaces (root groups / application instances)")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct NamespaceCommand {
    #[command(subcommand)]
    pub subcommand: NamespaceSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum NamespaceSubCommands {
    Create(create::CreateCommand),
    Get(get::GetCommand),
    #[command(alias = "del")]
    Delete(delete::DeleteCommand),
    Invite(invite::InviteCommand),
    Join(join::JoinCommand),
    /// Voluntarily leave a namespace (cascades through descendant groups).
    Leave(leave::LeaveCommand),
    Groups(groups::GroupsCommand),
    CreateGroup(create_group::CreateGroupCommand),
    #[command(alias = "ls")]
    List(list::ListCommand),
    Outbound(outbound::OutboundCommand),
}

impl NamespaceCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        crate::cli::dispatch_subcommands!(
            self.subcommand,
            environment,
            NamespaceSubCommands::Create,
            NamespaceSubCommands::Get,
            NamespaceSubCommands::Delete,
            NamespaceSubCommands::Invite,
            NamespaceSubCommands::Join,
            NamespaceSubCommands::Leave,
            NamespaceSubCommands::Groups,
            NamespaceSubCommands::CreateGroup,
            NamespaceSubCommands::List,
            NamespaceSubCommands::Outbound,
        )
    }
}
