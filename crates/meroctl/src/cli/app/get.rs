use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::GetApplicationResponse;
use clap::{Parser, ValueEnum};
use eyre::Result;

use crate::cli::Environment;
use crate::output::Report;

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

impl Report for GetApplicationResponse {
    fn report(&self) {
        match self.data.application {
            Some(ref application) => application.report(),
            None => println!("No application found"),
        }
    }
}

impl GetCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let mero_client = environment.mero_client()?;

        let response = mero_client.get_application(&self.app_id).await?;

        environment.output.write(&response);
        Ok(())
    }
}
