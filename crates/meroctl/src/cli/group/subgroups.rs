use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "List direct subgroups of a group")]
pub struct SubgroupsCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded parent group ID")]
    pub group_id: String,
}

impl SubgroupsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.list_subgroups(&self.group_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}
