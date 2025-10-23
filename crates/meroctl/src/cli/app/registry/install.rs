use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Install app from a registry")]
pub struct InstallCommand {
    /// App name to install
    #[arg(value_name = "APP_NAME", help = "Name of the app to install")]
    pub app_name: String,

    /// Registry name
    #[arg(long, short, help = "Registry name to install from")]
    pub registry: String,

    /// App version to install
    #[arg(long, short, help = "Version to install (defaults to latest)")]
    pub version: Option<String>,

    /// Metadata for the app
    #[arg(long, help = "Metadata for the app")]
    pub metadata: Option<String>,
}

impl InstallCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        // TODO: Implement registry app installation via API call
        // This would call POST /registries/{name}/apps/install endpoint
        println!(
            "Registry app installation not yet implemented: app={}, registry={}, version={:?}",
            self.app_name, self.registry, self.version
        );

        Ok(())
    }
}
