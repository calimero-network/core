use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

pub mod delete;
pub mod download;
pub mod info;
pub mod list;
pub mod upload;

pub const EXAMPLES: &str = r"
  # List all blobs
  $ meroctl --node node1 blob ls

  # Upload a blob from a file
  $ meroctl --node node1 blob upload --file /path/to/file

  # Upload a blob and announce to context for network discovery
  $ meroctl --node node1 blob upload --file /path/to/file --context-id <context_id>

  # Download a blob to a file
  $ meroctl --node node1 blob download --blob-id <blob_id> --output /path/to/output

  # Download a blob with network discovery
  $ meroctl --node node1 blob download --blob-id <blob_id> --output /path/to/output --context-id <context_id>

  # Get information about a specific blob
  $ meroctl --node node1 blob info --blob-id <blob_id>

  # Delete a specific blob
  $ meroctl --node node1 blob delete --blob-id <blob_id>
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
    #[command(about = "Upload a blob from a file")]
    Upload(upload::UploadCommand),
    #[command(about = "Download a blob to a file")]
    Download(download::DownloadCommand),
    #[command(about = "Get information about a specific blob")]
    Info(info::InfoCommand),
    #[command(about = "Delete a specific blob", alias = "rm")]
    Delete(delete::DeleteCommand),
}

impl BlobCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        crate::cli::dispatch_subcommands!(
            self.subcommand,
            environment,
            BlobSubCommands::Upload,
            BlobSubCommands::Download,
            BlobSubCommands::Delete,
            BlobSubCommands::Info,
            BlobSubCommands::List,
        )
    }
}
