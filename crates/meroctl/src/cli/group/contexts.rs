use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::DetachContextFromGroupApiRequest;
use clap::{Parser, Subcommand};
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Manage contexts within a group")]
pub struct ContextsCommand {
    #[command(subcommand)]
    pub subcommand: ContextsSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum ContextsSubCommands {
    #[command(alias = "ls", about = "List all contexts in a group")]
    List(ListGroupContextsCommand),
    #[command(about = "Detach a context from a group")]
    Detach(DetachContextCommand),
}

impl ContextsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            ContextsSubCommands::List(cmd) => cmd.run(environment).await,
            ContextsSubCommands::Detach(cmd) => cmd.run(environment).await,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "List all contexts in a group")]
pub struct ListGroupContextsCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,
}

impl ListGroupContextsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.list_group_contexts(&self.group_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Detach a context from a group")]
pub struct DetachContextCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "CONTEXT_ID", help = "The context ID (base58)")]
    pub context_id: ContextId,

    #[clap(long, help = "Public key of the requester (group admin)")]
    pub requester: PublicKey,

    #[clap(
        long,
        help = "Requester private key (hex). Deprecated: register a signing key instead"
    )]
    pub requester_secret: Option<String>,
}

impl DetachContextCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = DetachContextFromGroupApiRequest {
            requester: Some(self.requester),
            requester_secret: self.requester_secret,
        };

        let client = environment.client()?;
        let response = client
            .detach_context_from_group(&self.group_id, &self.context_id.to_string(), request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
