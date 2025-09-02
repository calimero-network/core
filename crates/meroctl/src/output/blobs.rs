use calimero_server_primitives::blob::{BlobDeleteResponse, BlobInfoResponse, BlobListResponse};
use comfy_table::{Cell, Color, Table};

use super::Report;

// Blob-related Report implementations
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

impl Report for BlobListResponse {
    fn report(&self) {
        if self.data.blobs.is_empty() {
            println!("No blobs found");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Blob ID").fg(Color::Blue),
                Cell::new("Size").fg(Color::Blue),
            ]);
            for blob in &self.data.blobs {
                let _ = table.add_row(vec![
                    blob.blob_id.to_string(),
                    format!("{} bytes", blob.size),
                ]);
            }
            println!("{table}");
        }
    }
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
