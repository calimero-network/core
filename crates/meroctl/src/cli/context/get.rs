use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::{OptionExt, Result};

use crate::cli::Environment;

#[derive(Copy, Clone, Parser, Debug)]
#[command(about = "Fetch details about the context")]
pub struct GetCommand {
    #[command(subcommand)]
    pub command: GetSubcommand,

    #[arg(
        value_name = "CONTEXT",
        help = "Context we're operating on",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,
}

#[derive(Copy, Clone, Debug, Parser)]
pub enum GetSubcommand {
    #[command(about = "Get context information")]
    Info,

    #[command(about = "Get client keys")]
    ClientKeys,

    #[command(about = "Get storage information")]
    Storage,
}











impl GetCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let resolve_response = client.resolve_alias(self.context, None).await?;
        let context_id = resolve_response
            .value()
            .copied()
            .ok_or_eyre("unable to resolve")?;

        match self.command {
            GetSubcommand::Info => {
                let response = client.get_context(&context_id).await?;
                environment.output.write(&response);
            }
            GetSubcommand::ClientKeys => {
                let response = client.get_context_client_keys(&context_id).await?;
                environment.output.write(&response);
            }
            GetSubcommand::Storage => {
                let response = client.get_context_storage(&context_id).await?;
                environment.output.write(&response);
            }
        }

        Ok(())
    }
}
