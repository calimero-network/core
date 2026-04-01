use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod identity;
pub mod list;

pub const EXAMPLES: &str = r"
  # List all namespaces (root groups)
  $ meroctl --node node1 namespace ls

  # Get namespace identity
  $ meroctl --node node1 namespace identity <namespace_id>
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
    #[command(alias = "ls")]
    List(list::ListCommand),
    #[command(alias = "id")]
    Identity(identity::IdentityCommand),
}

impl NamespaceCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            NamespaceSubCommands::List(cmd) => cmd.run(environment).await,
            NamespaceSubCommands::Identity(cmd) => cmd.run(environment).await,
        }
    }
}
