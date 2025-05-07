use calimero_server_primitives::admin::GetPeersCountResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::{eyre, Result as EyreResult};
use reqwest::Client;

use super::ConnectionInfo;
use crate::cli::Environment;
use crate::common::{do_request, multiaddr_to_url, RequestType};
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
        let (url, keypair) = match &environment.connection {
            Some(ConnectionInfo::Local { config, multiaddr }) => (
                multiaddr_to_url(multiaddr, "admin-api/dev/peers")?,
                Some(&config.identity),
            ),
            Some(ConnectionInfo::Remote { api }) => {
                let mut url = api.clone();
                url.set_path("admin-api/dev/peers");
                (url, None)
            }
            None => return Err(eyre!("No connection configured")),
        };

        let response: GetPeersCountResponse =
            do_request(&Client::new(), url, None::<()>, keypair, RequestType::Get).await?;

        environment.output.write(&response);

        Ok(())
    }
}
