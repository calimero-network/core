use calimero_primitives::application::ApplicationId;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Copy, Clone, Parser, Debug)]
#[command(about = "Uninstall an application")]
pub struct UninstallCommand {
    /// Application ID to uninstall
    #[arg(value_name = "APP_ID", help = "application_id of the application")]
    pub app_id: ApplicationId,
}

impl UninstallCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = client.uninstall_application(&self.app_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_uninstall_command_parsing_valid_app_id() {
        let app_id = ApplicationId::from([42u8; 32]);

        let cmd = UninstallCommand::try_parse_from(["uninstall", &app_id.to_string()]).unwrap();

        assert_eq!(cmd.app_id, app_id);
    }

    #[test]
    fn test_uninstall_command_missing_app_id_fails() {
        let result = UninstallCommand::try_parse_from(["uninstall"]);
        assert!(
            result.is_err(),
            "Command should fail when app_id is missing"
        );
    }

    #[test]
    fn test_uninstall_command_invalid_app_id_fails() {
        let result = UninstallCommand::try_parse_from(["uninstall", "invalid-app-id"]);
        assert!(
            result.is_err(),
            "Command should fail with invalid application ID"
        );
    }
}
