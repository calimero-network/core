use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Parser, Debug)]
#[command(about = "Get the latest version of a package")]
pub struct GetLatestVersionCommand {
    #[arg(help = "Package name (e.g., com.example.myapp)")]
    pub package: String,
}

impl GetLatestVersionCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = client.get_latest_version(&self.package).await?;

        environment.output.write(&response);
        Ok(())
    }
}
