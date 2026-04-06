use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Join a context via group membership (no invitation needed)")]
pub struct JoinContextCommand {
    #[clap(name = "CONTEXT_ID", help = "The context ID to join")]
    pub context_id: String,
}

impl JoinContextCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.join_context(&self.context_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}
