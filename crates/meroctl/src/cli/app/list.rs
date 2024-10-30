use std::fmt::Display;

use calimero_server_primitives::admin::ListApplicationsResponse;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;
use serde::Serialize;

use crate::cli::app::ApplicationReport;
use crate::cli::CommandContext;
use crate::common::{
    craft_failed_request_message, fetch_multiaddr, get_response, load_config, multiaddr_to_url,
    RequestType,
};

#[derive(Debug, Parser)]
#[command(about = "List installed applications")]
pub struct ListCommand;

#[derive(Debug, Serialize)]
struct OutputReport(ListApplicationsResponse);

impl Display for OutputReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for app in self.0.data.apps.iter() {
            let app_report = ApplicationReport(app.clone());
            write!(f, "{}", app_report)?;
        }
        Ok(())
    }
}

impl ListCommand {
    pub async fn run(self, context: CommandContext) -> EyreResult<()> {
        let config = load_config(&context.args.home, &context.args.node_name)?;

        let response = get_response(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/applications")?,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        if !response.status().is_success() {
            bail!(craft_failed_request_message(response, "Application list failed").await?)
        }

        let response = response.json::<ListApplicationsResponse>().await?;

        context.output.write_output(OutputReport(response));

        Ok(())
    }
}
