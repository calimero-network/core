use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Remove a registry")]
pub struct RemoveCommand {
    /// Registry name to remove
    #[arg(value_name = "REGISTRY_NAME", help = "Name of the registry to remove")]
    pub name: String,
}

impl RemoveCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        // TODO: Implement registry removal via API call
        // This would call DELETE /registries/{name} endpoint
        println!("Registry removal not yet implemented: name={}", self.name);

        Ok(())
    }
}
