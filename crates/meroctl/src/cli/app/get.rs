use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::GetApplicationResponse;
use clap::{Parser, ValueEnum};
use comfy_table::{Cell, Color, Table};
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
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

    fn pretty_report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Application").fg(Color::Blue)]);

        match &self.data.application {
            Some(app) => app.pretty_report(),
            None => {
                let _ = table.add_row(vec!["No application found"]);
                println!("{table}");
            }
        }
    }
}

impl GetCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            &format!("admin-api/dev/applications/{}", self.app_id),
        )?;

        let response: GetApplicationResponse = do_request(
            &Client::new(),
            url,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
