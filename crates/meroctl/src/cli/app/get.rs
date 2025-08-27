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
