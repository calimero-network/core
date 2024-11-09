use calimero_server_primitives::admin::GetContextsResponse;
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "List all contexts")]
pub struct ListCommand;

impl Report for GetContextsResponse {
    fn report(&self) {
        for context in &self.data.contexts {
            context.report();
        }
    }
}

impl ListCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let response: GetContextsResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/contexts")?,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
