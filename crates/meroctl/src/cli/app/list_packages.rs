use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Copy, Clone, Parser, Debug)]
#[command(about = "List all packages")]
pub struct ListPackagesCommand;

impl ListPackagesCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = client.list_packages().await?;

        environment.output.write(&response);
        Ok(())
    }
}
