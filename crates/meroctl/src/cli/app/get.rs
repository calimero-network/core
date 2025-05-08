use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::GetApplicationResponse;
use clap::{Parser, ValueEnum};
use eyre::{eyre, Result as EyreResult};
use reqwest::Client;

use crate::cli::{ConnectionInfo, Environment};
use crate::common::{do_request, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Parser, Debug)]
#[command(about = "Fetch application details")]
pub struct GetCommand {
    #[arg(value_name = "APP_ID", help = "application_id of the application")]
    pub app_id: ApplicationId,
}

#[derive(ValueEnum, Debug, Clone)]
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
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let (url, keypair) = match &environment.connection {
            Some(ConnectionInfo::Local { config, multiaddr }) => (
                multiaddr_to_url(
                    multiaddr,
                    &format!("admin-api/dev/applications/{}", self.app_id),
                )?,
                Some(&config.identity),
            ),
            Some(ConnectionInfo::Remote { api }) => {
                let mut url = api.clone();
                url.set_path(&format!("admin-api/dev/applications/{}", self.app_id));
                (url, None)
            }
            None => return Err(eyre!("No connection configured")),
        };

        let response: GetApplicationResponse =
            do_request(&Client::new(), url, None::<()>, keypair, RequestType::Get).await?;

        environment.output.write(&response);

        Ok(())
    }
}
