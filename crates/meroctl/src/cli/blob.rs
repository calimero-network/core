use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod delete;
pub mod info;
pub mod list;
pub mod upload;

pub const EXAMPLES: &str = r"
  # List all blobs
  $ meroctl --node node1 blob ls

  # Get information about a specific blob
  $ meroctl --node node1 blob info <blob_id>

  # Delete a specific blob
  $ meroctl --node node1 blob delete <blob_id>

  # Upload a blob from a file
  $ meroctl --node node1 blob upload --path /path/to/file.wasm

  # Upload a blob from a URL
  $ meroctl --node node1 blob upload --url https://example.com/file.wasm

  # Upload a blob from stdin
  $ cat file.wasm | meroctl --node node1 blob upload --stdin

  # Upload with hash verification
  $ meroctl --node node1 blob upload --path file.wasm --hash <expected_hash>
";

#[derive(Debug, Parser, Clone)]
#[command(about = "Command for managing blobs")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct BlobCommand {
    #[command(subcommand)]
    pub subcommand: BlobSubCommands,
}

#[derive(Debug, Subcommand, Clone)]
pub enum BlobSubCommands {
    #[command(about = "List all blobs", alias = "ls")]
    List(list::ListCommand),
    #[command(about = "Get information about a specific blob")]
    Info(info::InfoCommand),
    #[command(about = "Delete a specific blob", alias = "rm")]
    Delete(delete::DeleteCommand),
    #[command(about = "Upload a blob")]
    Upload(upload::UploadCommand),
}

impl BlobCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        match self.subcommand {
            BlobSubCommands::Delete(delete) => delete.run(environment).await,
            BlobSubCommands::Info(info) => info.run(environment).await,
            BlobSubCommands::List(list) => list.run(environment).await,
            BlobSubCommands::Upload(upload) => upload.run(environment).await,
        }
    }
}
