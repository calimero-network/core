use calimero_server_primitives::admin::GetPeersCountResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

pub const EXAMPLES: &str = r"
  #
  $ meroctl -- --node-name node1 peers
";

#[derive(Debug, Parser)]
#[command(about = "Return the number of connected peers")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct PeersCommand;

impl Report for GetPeersCountResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Connected Peers").fg(Color::Blue)]);
        let _ = table.add_row(vec![self.count.to_string()]);
        println!("{table}");
    }
}

impl PeersCommand {
    pub async fn run(&self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name).await?;

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
