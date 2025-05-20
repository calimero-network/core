use calimero_server_primitives::admin::GetPeersCountResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::{eyre, Result as EyreResult};
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, RequestType};
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
        let connection = environment
            .connection
            .as_ref()
            .ok_or_else(|| eyre!("No connection configured"))?;

        let mut url = connection.api_url.clone();
        url.set_path("admin-api/dev/peers");

        let keypair = connection
            .auth_key
            .as_ref()
            .and_then(|k| bs58::decode(k).into_vec().ok())
            .and_then(|bytes| libp2p::identity::Keypair::from_protobuf_encoding(&bytes).ok());

        let response: GetPeersCountResponse = do_request(
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
