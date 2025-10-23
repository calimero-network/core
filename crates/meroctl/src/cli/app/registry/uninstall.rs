use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Uninstall app from a registry")]
pub struct UninstallCommand {
    /// App name to uninstall
    #[arg(value_name = "APP_NAME", help = "Name of the app to uninstall")]
    pub app_name: String,

    /// Registry name
    #[arg(long, short, help = "Registry name to uninstall from")]
    pub registry: String,
}

impl UninstallCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        // TODO: Implement registry app uninstallation via API call
        // This would call DELETE /registries/{name}/apps/uninstall endpoint
        println!(
            "Registry app uninstallation not yet implemented: app={}, registry={}",
            self.app_name, self.registry
        );

        Ok(())
    }
}
