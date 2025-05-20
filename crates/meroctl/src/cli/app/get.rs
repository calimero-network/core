use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::GetApplicationResponse;
use clap::{Parser, ValueEnum};
use eyre::{eyre, Result as EyreResult};
use libp2p::identity::Keypair;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, RequestType};
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
        let connection = environment
            .connection
            .as_ref()
            .ok_or_else(|| eyre!("No connection configured"))?;

        let mut url = connection.api_url.clone();
        url.set_path(&format!("admin-api/dev/applications/{}", self.app_id));

        let keypair = connection
            .auth_key
            .as_ref()
            .and_then(|k| bs58::decode(k).into_vec().ok())
            .and_then(|bytes| Keypair::from_protobuf_encoding(&bytes).ok());

        let response: GetApplicationResponse = do_request(
            &Client::new(),
            url,
            None::<()>,
            keypair.as_ref(),
            RequestType::Get,
        )
        .await?;

        environment.output.write(&response);
        Ok(())
    }
}
