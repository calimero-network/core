use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "List apps from a registry")]
pub struct ListCommand {
    /// Registry name
    #[arg(long, short, help = "Registry name to list apps from")]
    pub registry: String,

    /// Filter by developer
    #[arg(long, help = "Filter apps by developer")]
    pub developer: Option<String>,

    /// Filter by app name
    #[arg(long, help = "Filter apps by name")]
    pub name: Option<String>,
}

impl ListCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        // TODO: Implement registry app listing via API call
        // This would call GET /registries/{name}/apps endpoint
        println!(
            "Registry app listing not yet implemented: registry={}, developer={:?}, name={:?}",
            self.registry, self.developer, self.name
        );

        Ok(())
    }
}
