use std::fs;
use std::path::PathBuf;

use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::{eyre, Result};

use crate::cli::Environment;
use crate::output::BlobUploadResponse;

#[derive(Clone, Debug, Parser)]
#[command(about = "Upload a blob from a file")]
pub struct UploadCommand {
    #[arg(
        short = 'f',
        long = "file",
        value_name = "FILE",
        help = "Path to the file to upload"
    )]
    pub file_path: PathBuf,

    #[arg(
        short = 'c',
        long = "context-id",
        value_name = "CONTEXT_ID",
        help = "Optional context ID to announce the blob to for network discovery"
    )]
    pub context_id: Option<ContextId>,
}

impl UploadCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        // Read the file
        let data = fs::read(&self.file_path)
            .map_err(|e| eyre!("Failed to read file '{}': {}", self.file_path.display(), e))?;

        // Upload the blob
        let blob_info = client.upload_blob(data, self.context_id.as_ref()).await?;

        let response = BlobUploadResponse::from(blob_info);
        environment.output.write(&response);

        Ok(())
    }
}
