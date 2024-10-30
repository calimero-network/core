use std::fmt::Display;

use calimero_server_primitives::admin::GetApplicationResponse;
use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult};
use reqwest::Client;
use serde::Serialize;

use crate::cli::app::ApplicationReport;
use crate::cli::CommandContext;
use crate::common::{
    craft_failed_request_message, fetch_multiaddr, get_response, load_config, multiaddr_to_url,
    RequestType,
};

#[derive(Parser, Debug)]
#[command(about = "Fetch application details")]
pub struct GetCommand {
    #[arg(value_name = "APP_ID", help = "application_id of the application")]
    pub app_id: String,
}

#[derive(ValueEnum, Debug, Clone)]
pub enum GetValues {
    Details,
}

#[derive(Debug, Serialize)]
struct OutputReport(GetApplicationResponse);

impl Display for OutputReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.data.application {
            Some(ref application) => {
                write!(f, "{}", ApplicationReport(application))
            }
            None => write!(f, "No application found"),
        }
    }
}

impl GetCommand {
    #[expect(clippy::print_stdout, reason = "Acceptable for CLI")]
    pub async fn run(self, context: CommandContext) -> EyreResult<()> {
        let config = load_config(&context.args.home, &context.args.node_name)?;

        let url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            &format!("admin-api/dev/applications/{}", self.app_id),
        )?;

        let response = get_response(
            &Client::new(),
            url,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        if !response.status().is_success() {
            bail!(craft_failed_request_message(response, "Application get failed").await?)
        }

        let response = response.json::<GetApplicationResponse>().await?;

        context.output.write_output(OutputReport(response));

        Ok(())
    }
}
