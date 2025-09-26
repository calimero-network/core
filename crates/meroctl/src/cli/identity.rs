use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod add;
pub mod export;
pub mod generate;
pub mod list;
pub mod remove;

pub const EXAMPLES: &str = r"
  # Generate a new identity
  $ meroctl identity generate

  # List all identities
  $ meroctl identity list

  # Export an identity
  $ meroctl identity export <public_key>

  # Remove an identity
  $ meroctl identity remove <public_key>
";

#[derive(Debug, Parser)]
#[command(about = "Manage global identities")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct IdentityCommand {
    #[command(subcommand)]
    pub subcommand: IdentitySubCommands,
}

#[derive(Debug, Subcommand)]
pub enum IdentitySubCommands {
    #[command(about = "Add/import an identity", alias = "import")]
    Add(add::AddCommand),
    #[command(about = "Export an identity")]
    Export(export::ExportCommand),
    #[command(about = "Generate a new identity", alias = "new")]
    Generate(generate::GenerateCommand),
    #[command(about = "List identities", alias = "ls")]
    List(list::ListCommand),
    #[command(about = "Remove an identity", aliases = ["rm", "del", "delete"])]
    Remove(remove::RemoveCommand),
}

impl IdentityCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            IdentitySubCommands::Add(add) => add.run(environment).await,
            IdentitySubCommands::Export(export) => export.run(environment).await,
            IdentitySubCommands::Generate(generate) => generate.run(environment).await,
            IdentitySubCommands::List(list) => list.run(environment).await,
            IdentitySubCommands::Remove(remove) => remove.run(environment).await,
        }
    }
}
