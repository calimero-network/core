use calimero_primitives::application::ApplicationId;
use clap::{Parser, ValueEnum};
use eyre::Result;

use crate::cli::Environment;

#[derive(Copy, Clone, Parser, Debug)]
#[command(about = "Fetch application details")]
pub struct GetCommand {
    #[arg(value_name = "APP_ID", help = "application_id of the application")]
    pub app_id: ApplicationId,
}

#[derive(Copy, ValueEnum, Debug, Clone)]
pub enum GetValues {
    Details,
}

impl GetCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = client.get_application(&self.app_id).await?;

        environment.output.write(&response);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_get_command_parsing_valid_app_id() {
        let app_id = ApplicationId::from([42u8; 32]);
        let cmd = GetCommand::try_parse_from(["get", &app_id.to_string()]).unwrap();

        assert_eq!(cmd.app_id, app_id);
    }

    #[test]
    fn test_get_command_missing_app_id_fails() {
        let result = GetCommand::try_parse_from(["get"]);
        assert!(
            result.is_err(),
            "Command should fail when app_id is missing"
        );
    }

    #[test]
    fn test_get_command_invalid_app_id_fails() {
        let result = GetCommand::try_parse_from(["get", "invalid-app-id"]);
        assert!(
            result.is_err(),
            "Command should fail with invalid application ID"
        );
    }

    #[test]
    fn test_get_command_short_app_id_fails() {
        // Application ID must be exactly 32 bytes when decoded
        let result = GetCommand::try_parse_from(["get", "abc123"]);
        assert!(result.is_err(), "Command should fail with short app ID");
    }
}
