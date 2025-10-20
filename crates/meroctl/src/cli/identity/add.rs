use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Add/import an identity")]
pub struct AddCommand {
    #[arg(help = "JSON data of the identity to import")]
    pub json_data: String,
}

impl AddCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.import_identity(&self.json_data).await?;
        environment.output.write(&response);
        Ok(())
    }
}
