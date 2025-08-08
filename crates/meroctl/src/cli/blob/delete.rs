use calimero_primitives::blobs::BlobId;
use clap::Parser;
use eyre::Result;
use serde::{Deserialize, Serialize};

use crate::cli::Environment;
use crate::output::Report;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Delete a blob by its ID")]
pub struct DeleteCommand {
    #[arg(value_name = "BLOB_ID", help = "ID of the blob to delete")]
    pub blob_id: BlobId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobDeleteResponse {
    pub blob_id: BlobId,
    pub deleted: bool,
}

impl Report for BlobDeleteResponse {
    fn report(&self) {
        if self.deleted {
            println!("Successfully deleted blob '{}'", self.blob_id);
        } else {
            println!(
                "Failed to delete blob '{}' (blob may not exist)",
                self.blob_id
            );
        }
    }
}

impl DeleteCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let connection = environment.connection();

        let response: BlobDeleteResponse = connection
            .delete(&format!("admin-api/blobs/{}", self.blob_id))
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
