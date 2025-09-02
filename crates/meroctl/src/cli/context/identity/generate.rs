use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Generate public/private key pair used for context identity")]
pub struct GenerateCommand;

impl GenerateCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = client.generate_context_identity().await?;

        environment.output.write(&response);
        Ok(())
    }
}
