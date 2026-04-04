use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "List direct groups under a namespace")]
pub struct GroupsCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace (root group) ID")]
    pub namespace_id: String,
}

impl GroupsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.list_namespace_groups(&self.namespace_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}
