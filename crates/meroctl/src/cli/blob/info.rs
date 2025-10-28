use calimero_primitives::blobs::BlobId;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser, Copy)]
#[command(about = "Get information about a blob")]
pub struct InfoCommand {
    #[arg(
        short = 'b',
        long = "blob-id",
        value_name = "BLOB_ID",
        help = "ID of the blob to get info for"
    )]
    pub blob_id: BlobId,
}

impl InfoCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let blob_info = client.get_blob_info(&self.blob_id).await?;

        environment.output.write(&blob_info);

        Ok(())
    }
}
