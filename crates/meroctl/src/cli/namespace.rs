use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod create;
pub mod create_group;
pub mod delete;
pub mod get;
pub mod groups;
pub mod identity;
pub mod invite;
pub mod join;
pub mod list;

pub const EXAMPLES: &str = r"
  # Create a namespace (root group)
  $ meroctl --node node1 namespace create --application-id <app_id>

  # List all namespaces
  $ meroctl --node node1 namespace ls

  # Get and delete a namespace
  $ meroctl --node node1 namespace get <namespace_id>
  $ meroctl --node node1 namespace delete <namespace_id>

  # Invite another node and join namespace
  $ meroctl --node node1 namespace invite <namespace_id>
  $ meroctl --node node2 namespace join <namespace_id> '<invitation_json>'

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
    Groups(groups::GroupsCommand),
    CreateGroup(create_group::CreateGroupCommand),
    #[command(alias = "ls")]
    List(list::ListCommand),
    #[command(alias = "id")]
    Identity(identity::IdentityCommand),
}

impl NamespaceCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            NamespaceSubCommands::Create(cmd) => cmd.run(environment).await,
            NamespaceSubCommands::Get(cmd) => cmd.run(environment).await,
            NamespaceSubCommands::Delete(cmd) => cmd.run(environment).await,
            NamespaceSubCommands::Invite(cmd) => cmd.run(environment).await,
            NamespaceSubCommands::Join(cmd) => cmd.run(environment).await,
            NamespaceSubCommands::Groups(cmd) => cmd.run(environment).await,
            NamespaceSubCommands::CreateGroup(cmd) => cmd.run(environment).await,
            NamespaceSubCommands::List(cmd) => cmd.run(environment).await,
            NamespaceSubCommands::Identity(cmd) => cmd.run(environment).await,
        }
    }
}
