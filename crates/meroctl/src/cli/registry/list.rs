use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "List all configured registries")]
pub struct ListCommand;

impl ListCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        // TODO: Implement registry listing via API call
        // This would call GET /registries endpoint
        println!("Registry listing not yet implemented");

        Ok(())
    }
}
