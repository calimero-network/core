use std::fs;
use std::path::PathBuf;

use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::{eyre, Result};

use crate::cli::validation::validate_parent_directory_exists;
use crate::cli::Environment;
use crate::output::BlobDownloadResponse;

#[derive(Clone, Debug, Parser)]
#[command(about = "Download a blob to a file")]
pub struct DownloadCommand {
    #[arg(
        short = 'b',
        long = "blob-id",
        value_name = "BLOB_ID",
        help = "ID of the blob to download"
    )]
    pub blob_id: BlobId,

    #[arg(
        short = 'o',
        long = "output",
        value_name = "OUTPUT",
        help = "Path where the file should be saved"
    )]
    pub output_path: PathBuf,

    #[arg(
        short = 'c',
        long = "context-id",
        value_name = "CONTEXT_ID",
        help = "Optional context ID to search for the blob in the network"
    )]
    pub context_id: Option<ContextId>,
}

impl DownloadCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        // Validate output path before downloading
        validate_parent_directory_exists(&self.output_path)?;

        let client = environment.client()?;

        // Download the blob
        let data = client
            .download_blob(&self.blob_id, self.context_id.as_ref())
            .await?;

        // Get file size before writing
        let size = data.len() as u64;

        // Write to file
        fs::write(&self.output_path, data).map_err(|e| {
            eyre!(
                "Failed to write file '{}': {}",
                self.output_path.display(),
                e
            )
        })?;

        let response = BlobDownloadResponse {
            blob_id: self.blob_id,
            output_path: self.output_path,
            size,
        };

        environment.output.write(&response);

        Ok(())
    }
}
