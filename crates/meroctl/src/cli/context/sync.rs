use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::{OptionExt, Result};

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Explicitly request a sync")]
pub struct SyncCommand {
    #[clap(long, short, help = "The context to sync", default_value = "default")]
    context: Alias<ContextId>,

    #[clap(long, short, help = "Sync all contexts", conflicts_with = "context")]
    all: bool,
}



impl SyncCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let response = if self.all {
            client.sync_all_contexts().await?
        } else {
            let context_id = client
                .resolve_alias(self.context, None)
                .await?
                .value()
                .copied()
                .ok_or_eyre("unable to resolve")?;

            client.sync_context(&context_id).await?
        };

        environment.output.write(&response);

        Ok(())
    }
}
