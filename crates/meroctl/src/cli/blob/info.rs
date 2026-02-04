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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_info_command_parsing_valid_blob_id() {
        let blob_id = BlobId::from([42u8; 32]);

        let cmd = InfoCommand::try_parse_from(["info", "--blob-id", &blob_id.to_string()]).unwrap();

        assert_eq!(cmd.blob_id, blob_id);
    }

    #[test]
    fn test_info_command_parsing_short_flag() {
        let blob_id = BlobId::from([42u8; 32]);

        let cmd = InfoCommand::try_parse_from(["info", "-b", &blob_id.to_string()]).unwrap();

        assert_eq!(cmd.blob_id, blob_id);
    }

    #[test]
    fn test_info_command_missing_blob_id_fails() {
        let result = InfoCommand::try_parse_from(["info"]);
        assert!(
            result.is_err(),
            "Command should fail when blob_id is missing"
        );
    }

    #[test]
    fn test_info_command_invalid_blob_id_fails() {
        let result = InfoCommand::try_parse_from(["info", "--blob-id", "invalid-blob-id"]);
        assert!(result.is_err(), "Command should fail with invalid blob ID");
    }
}
