use calimero_primitives::blobs::{BlobId, BlobMetadata};
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::Result;
use serde::{Deserialize, Serialize};

use crate::cli::Environment;
use crate::output::Report;

#[derive(Clone, Debug, Parser, Copy)]
#[command(about = "Get information about a blob")]
pub struct InfoCommand {
    #[arg(value_name = "BLOB_ID", help = "ID of the blob to get info for")]
    pub blob_id: BlobId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobInfoResponse {
    pub data: BlobMetadata,
}

impl Report for BlobInfoResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Blob ID").fg(Color::Blue),
            Cell::new("Size (bytes)").fg(Color::Blue),
            Cell::new("MIME Type").fg(Color::Blue),
            Cell::new("Hash").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec![
            &self.data.blob_id.to_string(),
            &self.data.size.to_string(),
            &self.data.mime_type,
            &hex::encode(self.data.hash),
        ]);

        println!("{table}");
    }
}

impl InfoCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let connection = environment.connection();

        let headers = connection
            .head(&format!("admin-api/blobs/{}", self.blob_id))
            .await?;

        let size = headers
            .get("content-length")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let mime_type = headers
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();

        let hash_hex = headers
            .get("x-blob-hash")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");

        let hash =
            hex::decode(hash_hex).map_err(|_| eyre::eyre!("Invalid hash in response headers"))?;

        let hash_array: [u8; 32] = hash
            .try_into()
            .map_err(|_| eyre::eyre!("Hash must be 32 bytes"))?;

        let blob_info = BlobInfoResponse {
            data: BlobMetadata {
                blob_id: self.blob_id,
                size,
                mime_type,
                hash: hash_array,
            },
        };

        environment.output.write(&blob_info);

        Ok(())
    }
}
