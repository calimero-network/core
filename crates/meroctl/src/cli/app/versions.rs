use calimero_primitives::application::ApplicationId;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Parser, Debug)]
#[command(
    about = "List locally installed versions of an application (the latest \
             install plus older bytecode still referenced by namespaces)"
)]
pub struct VersionsCommand {
    #[arg(help = "The application ID")]
    pub app_id: ApplicationId,
}

impl VersionsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = client.list_application_versions(&self.app_id).await?;

        environment.output.write(&response);
        Ok(())
    }
}
