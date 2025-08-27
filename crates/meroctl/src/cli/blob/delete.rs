use calimero_primitives::blobs::BlobId;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Delete a blob by its ID")]
pub struct DeleteCommand {
    #[arg(value_name = "BLOB_ID", help = "ID of the blob to delete")]
    pub blob_id: BlobId,
}

impl DeleteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = client.delete_blob(&self.blob_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}
