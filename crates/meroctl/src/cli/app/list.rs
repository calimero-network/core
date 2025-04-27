use calimero_server_primitives::admin::ListApplicationsResponse;
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "List installed applications")]
pub struct ListCommand;

impl Report for ListApplicationsResponse {
    fn report(&self) {
        for application in &self.data.apps {
            application.report();
        }
    }
}

impl ListCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(
            &environment.args.home,
            environment.args.node_name.as_deref().unwrap_or_default(),
        )?;

        let response: ListApplicationsResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/applications")?,
            None::<()>,
            Some(&config.identity),
            RequestType::Get,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
