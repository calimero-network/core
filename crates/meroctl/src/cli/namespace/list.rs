use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "List all namespaces (root groups / application instances)")]
pub struct ListCommand;

impl ListCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.list_namespaces().await?;

        environment.output.write(&response);

        Ok(())
    }
}
