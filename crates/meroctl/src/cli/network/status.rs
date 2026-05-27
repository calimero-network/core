use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Snapshot libp2p connectivity: relays, rendezvous, DCUtR, AutoNAT")]
pub struct StatusCommand;

impl StatusCommand {
    pub async fn run(&self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.network_status().await?;
        environment.output.write(&response);
        Ok(())
    }
}
