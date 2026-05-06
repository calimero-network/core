use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Locally opt out of a context (no DAG op, no broadcast). \
             Stops sync and disarms auto-follow on this node only — \
             peers do not observe the leave. Reverse with `group join-context`.")]
pub struct LeaveContextCommand {
    #[clap(name = "CONTEXT_ID", help = "The context ID to leave locally")]
    pub context_id: String,
}

impl LeaveContextCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.leave_context(&self.context_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}
