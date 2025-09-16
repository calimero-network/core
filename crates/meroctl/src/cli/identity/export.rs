use calimero_primitives::identity::PublicKey;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Export an identity")]
pub struct ExportCommand {
    #[arg(help = "Public key of the identity to export")]
    pub public_key: PublicKey,
}

impl ExportCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.export_identity(&self.public_key).await?;
        environment.output.write(&response);
        Ok(())
    }
}
