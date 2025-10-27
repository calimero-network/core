use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Parser, Debug)]
#[command(about = "List versions of a package")]
pub struct ListVersionsCommand {
    #[arg(help = "Package name (e.g., com.example.myapp)")]
    pub package: String,
}

impl ListVersionsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = client.list_versions(&self.package).await?;

        environment.output.write(&response);
        Ok(())
    }
}
