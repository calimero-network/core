use std::fs;
use std::path::PathBuf;

use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::{eyre, Result};

use crate::cli::validation::existing_file_path;
use crate::cli::Environment;
use crate::output::BlobUploadResponse;

#[derive(Clone, Debug, Parser)]
#[command(about = "Upload a blob from a file")]
pub struct UploadCommand {
    #[arg(
        short = 'f',
        long = "file",
        value_name = "FILE",
        help = "Path to the file to upload",
        value_parser = existing_file_path
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_upload_command_parsing_with_existing_file() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "test content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let cmd = UploadCommand::try_parse_from(["upload", "--file", path]).unwrap();

        assert_eq!(cmd.file_path, PathBuf::from(path));
        assert!(cmd.context_id.is_none());
    }

    #[test]
    fn test_upload_command_parsing_short_flag() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "test content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let cmd = UploadCommand::try_parse_from(["upload", "-f", path]).unwrap();

        assert_eq!(cmd.file_path, PathBuf::from(path));
    }

    #[test]
    fn test_upload_command_parsing_with_context_id() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "test content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let context_id = ContextId::from([1u8; 32]);
        let cmd = UploadCommand::try_parse_from([
            "upload",
            "--file",
            path,
            "--context-id",
            &context_id.to_string(),
        ])
        .unwrap();

        assert_eq!(cmd.file_path, PathBuf::from(path));
        assert_eq!(cmd.context_id, Some(context_id));
    }

    #[test]
    fn test_upload_command_missing_file_flag_fails() {
        let result = UploadCommand::try_parse_from(["upload"]);
        assert!(
            result.is_err(),
            "Command should fail when --file is missing"
        );
    }

    #[test]
    fn test_upload_command_nonexistent_file_fails() {
        let result =
            UploadCommand::try_parse_from(["upload", "--file", "/nonexistent/path/file.wasm"]);
        assert!(
            result.is_err(),
            "Command should fail when file doesn't exist"
        );
    }

    #[test]
    fn test_upload_command_invalid_context_id_fails() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "test content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let result = UploadCommand::try_parse_from([
            "upload",
            "--file",
            path,
            "--context-id",
            "invalid-context-id",
        ]);
        assert!(
            result.is_err(),
            "Command should fail with invalid context ID"
        );
    }
}
