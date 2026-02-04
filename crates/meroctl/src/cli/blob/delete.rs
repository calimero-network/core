use calimero_primitives::blobs::BlobId;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Delete a blob by its ID")]
pub struct DeleteCommand {
    #[arg(
        short = 'b',
        long = "blob-id",
        value_name = "BLOB_ID",
        help = "ID of the blob to delete"
    )]
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_delete_command_parsing_valid_blob_id() {
        let blob_id = BlobId::from([42u8; 32]);

        let cmd =
            DeleteCommand::try_parse_from(["delete", "--blob-id", &blob_id.to_string()]).unwrap();

        assert_eq!(cmd.blob_id, blob_id);
    }

    #[test]
    fn test_delete_command_parsing_short_flag() {
        let blob_id = BlobId::from([42u8; 32]);

        let cmd = DeleteCommand::try_parse_from(["delete", "-b", &blob_id.to_string()]).unwrap();

        assert_eq!(cmd.blob_id, blob_id);
    }

    #[test]
    fn test_delete_command_missing_blob_id_fails() {
        let result = DeleteCommand::try_parse_from(["delete"]);
        assert!(
            result.is_err(),
            "Command should fail when blob_id is missing"
        );
    }

    #[test]
    fn test_delete_command_invalid_blob_id_fails() {
        let result = DeleteCommand::try_parse_from(["delete", "--blob-id", "invalid-blob-id"]);
        assert!(result.is_err(), "Command should fail with invalid blob ID");
    }
}
