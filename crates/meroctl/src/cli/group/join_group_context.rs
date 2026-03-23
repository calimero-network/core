use calimero_server_primitives::admin::JoinGroupContextApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Join a context via group membership (no invitation needed)")]
pub struct JoinGroupContextCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(long, help = "The context ID to join")]
    pub context_id: calimero_primitives::context::ContextId,
}

impl JoinGroupContextCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = JoinGroupContextApiRequest {
            context_id: self.context_id,
        };

        let client = environment.client()?;
        let response = client.join_group_context(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
