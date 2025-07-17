use calimero_primitives::blobs::BlobInfo;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::Result;
use serde::{Deserialize, Serialize};

use crate::cli::Environment;
use crate::output::Report;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "List all blobs")]
pub struct ListCommand;

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobListResponse {
    pub data: BlobListResponseData,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobListResponseData {
    pub blobs: Vec<BlobInfo>,
}

impl Report for BlobListResponse {
    fn report(&self) {
        if self.data.blobs.is_empty() {
            println!("No blobs found");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Blob ID").fg(Color::Blue),
                Cell::new("Size").fg(Color::Blue),
                Cell::new("Size (MB)").fg(Color::Blue),
            ]);
            for blob in &self.data.blobs {
                let size_mb = blob.size as f64 / (1024.0 * 1024.0);
                let _ = table.add_row(vec![
                    blob.blob_id.to_string(),
                    format!("{} bytes", blob.size),
                    format!("{:.2} MB", size_mb),
                ]);
            }
            println!("{table}");
        }
    }
}

impl ListCommand {
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let connection = environment.connection()?;

        let response: BlobListResponse = connection.get("admin-api/blobs").await?;

        environment.output.write(&response);

        Ok(())
    }
} 