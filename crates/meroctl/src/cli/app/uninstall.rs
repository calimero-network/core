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
