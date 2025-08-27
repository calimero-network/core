use clap::Parser;
use const_format::concatcp;
use eyre::Result;

use crate::cli::Environment;

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

impl PeersCommand {
    pub async fn run(&self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.get_peers_count().await?;

        environment.output.write(&response);

        Ok(())
    }
}
