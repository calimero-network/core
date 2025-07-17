use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use clap::{Parser, Subcommand};
use eyre::Result;
use owo_colors::OwoColorize;

#[derive(Debug, Parser, Copy, Clone)]
#[command(about = "Command for managing blobs")]
pub struct BlobCommand {
    #[command(subcommand)]
    pub subcommand: BlobSubCommand,
}

#[derive(Debug, Subcommand, Copy, Clone)]
pub enum BlobSubCommand {
    #[command(about = "List all blobs", alias = "ls")]
    List,
    #[command(about = "Get information about a specific blob")]
    Info { blob_id: BlobId },
    #[command(about = "Delete a specific blob")]
    Delete { blob_id: BlobId },
}

impl BlobCommand {
    pub async fn run(&self, node_client: &NodeClient) -> Result<()> {
        let ind = ">>".blue();

        match &self.subcommand {
            BlobSubCommand::List => match node_client.list_blobs() {
                Ok(blobs) => {
                    if blobs.is_empty() {
                        println!("{ind} No blobs found");
                    } else {
                        println!(
                            "{ind} {c1:44} | {c2:12}",
                            c1 = "Blob ID",
                            c2 = "Size (bytes)",
                        );

                        for blob in blobs {
                            let entry =
                                format!("{c1:44} | {c2:12}", c1 = blob.blob_id, c2 = blob.size,);

                            for line in entry.lines() {
                                println!("{ind} {}", line.cyan());
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("{ind} Failed to list blobs: {}", err);
                }
            },
            BlobSubCommand::Info { blob_id } => match node_client.get_blob_info(*blob_id).await {
                Ok(Some(metadata)) => {
                    println!(
                        "  {:<44} | {:<22} | {:<20} | {}",
                        "ID".cyan(),
                        "Size (bytes)".cyan(),
                        "MIME Type".cyan(),
                        "Hash".cyan()
                    );
                    println!("  {}", "-".repeat(130));
                    println!(
                        "  {:<44} | {:<22} | {:<20} | {}",
                        metadata.blob_id.to_string().cyan(),
                        format!("{}", metadata.size).cyan(),
                        metadata.mime_type.cyan(),
                        hex::encode(metadata.hash).cyan()
                    );
                }
                Ok(None) => {
                    println!("{ind} Blob '{}' not found", blob_id.cyan());
                }
                Err(err) => {
                    eprintln!("{ind} Failed to get blob info: {}", err);
                }
            },
            BlobSubCommand::Delete { blob_id } => match node_client.delete_blob(*blob_id).await {
                Ok(deleted) => {
                    if deleted {
                        println!("{ind} Successfully deleted blob '{}'", blob_id.cyan());
                    } else {
                        println!(
                            "{ind} Blob '{}' not found or already deleted",
                            blob_id.cyan()
                        );
                    }
                }
                Err(err) => {
                    eprintln!("{ind} Failed to delete blob: {}", err);
                }
            },
        }
        Ok(())
    }
}
