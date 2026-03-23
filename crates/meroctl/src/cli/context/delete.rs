use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::Parser;
use eyre::{OptionExt, Result};

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Delete a context")]
pub struct DeleteCommand {
    #[clap(name = "CONTEXT", help = "The context to delete")]
    pub context: Alias<ContextId>,

    #[clap(
        long,
        help = "Identity (public key) of the requester. Required when deleting a group context; must be a group admin."
    )]
    pub requester: Option<PublicKey>,
}

impl DeleteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let context_id = client
            .resolve_alias(self.context, None)
            .await?
            .value()
            .copied()
            .ok_or_eyre("unable to resolve")?;

        let response = client.delete_context(&context_id, self.requester).await?;

        environment.output.write(&response);

        Ok(())
    }
}
