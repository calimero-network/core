use calimero_server_primitives::admin::GetPeersCountResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;
use crate::output::Report;

pub const EXAMPLES: &str = r"
  $ meroctl --node node1 peers
";

#[derive(Copy, Clone, Debug, Parser)]
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
    pub async fn run(&self, environment: &mut Environment) -> Result<()> {
        let mero_client = environment.mero_client()?;
        let response = mero_client.get_peers_count().await?;

        environment.output.write(&response);

        Ok(())
    }
}
