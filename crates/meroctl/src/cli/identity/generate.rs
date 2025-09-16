use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Generate a new identity keypair")]
pub struct GenerateCommand {
    #[arg(help = "Alias for the new identity")]
    pub alias: Option<String>,
}

impl GenerateCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.generate_identity(self.alias).await?;
        environment.output.write(&response);
        Ok(())
    }
}
