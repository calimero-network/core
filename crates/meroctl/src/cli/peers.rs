use calimero_server_primitives::admin::GetPeersCountResponse;
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Debug, Parser)]
pub struct PeersCommand;

impl Report for GetPeersCountResponse {
    fn report(&self) {
        println!("{}", self.count);
    }
}

impl PeersCommand {
    pub async fn run(&self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let response: GetPeersCountResponse = do_request(
            &Client::new(),
            multiaddr_to_url(fetch_multiaddr(&config)?, "admin-api/dev/peers")?,
            None::<()>,
            &config.identity,
            RequestType::Get,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
