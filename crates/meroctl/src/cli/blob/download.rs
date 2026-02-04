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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use tempfile::tempdir;

    #[test]
    fn test_download_command_parsing_minimal() {
        let blob_id = BlobId::from([42u8; 32]);
        let temp_dir = tempdir().unwrap();
        let output_path = temp_dir.path().join("output.bin");

        let cmd = DownloadCommand::try_parse_from([
            "download",
            "--blob-id",
            &blob_id.to_string(),
            "--output",
            output_path.to_str().unwrap(),
        ])
        .unwrap();

        assert_eq!(cmd.blob_id, blob_id);
        assert_eq!(cmd.output_path, output_path);
        assert!(cmd.context_id.is_none());
    }

    #[test]
    fn test_download_command_parsing_short_flags() {
        let blob_id = BlobId::from([42u8; 32]);
        let temp_dir = tempdir().unwrap();
        let output_path = temp_dir.path().join("output.bin");

        let cmd = DownloadCommand::try_parse_from([
            "download",
            "-b",
            &blob_id.to_string(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .unwrap();

        assert_eq!(cmd.blob_id, blob_id);
        assert_eq!(cmd.output_path, output_path);
    }

    #[test]
    fn test_download_command_parsing_with_context_id() {
        let blob_id = BlobId::from([42u8; 32]);
        let context_id = ContextId::from([1u8; 32]);
        let temp_dir = tempdir().unwrap();
        let output_path = temp_dir.path().join("output.bin");

        let cmd = DownloadCommand::try_parse_from([
            "download",
            "--blob-id",
            &blob_id.to_string(),
            "--output",
            output_path.to_str().unwrap(),
            "--context-id",
            &context_id.to_string(),
        ])
        .unwrap();

        assert_eq!(cmd.blob_id, blob_id);
        assert_eq!(cmd.context_id, Some(context_id));
    }

    #[test]
    fn test_download_command_missing_blob_id_fails() {
        let temp_dir = tempdir().unwrap();
        let output_path = temp_dir.path().join("output.bin");

        let result = DownloadCommand::try_parse_from([
            "download",
            "--output",
            output_path.to_str().unwrap(),
        ]);
        assert!(
            result.is_err(),
            "Command should fail when --blob-id is missing"
        );
    }

    #[test]
    fn test_download_command_missing_output_fails() {
        let blob_id = BlobId::from([42u8; 32]);

        let result =
            DownloadCommand::try_parse_from(["download", "--blob-id", &blob_id.to_string()]);
        assert!(
            result.is_err(),
            "Command should fail when --output is missing"
        );
    }

    #[test]
    fn test_download_command_invalid_blob_id_fails() {
        let temp_dir = tempdir().unwrap();
        let output_path = temp_dir.path().join("output.bin");

        let result = DownloadCommand::try_parse_from([
            "download",
            "--blob-id",
            "invalid-blob-id",
            "--output",
            output_path.to_str().unwrap(),
        ]);
        assert!(result.is_err(), "Command should fail with invalid blob ID");
    }
}
