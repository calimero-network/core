use calimero_primitives::identity::PublicKey;
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

    #[clap(
        long,
        help = "Public key of the identity joining the context (defaults to node NEAR identity)"
    )]
    pub joiner_identity: Option<PublicKey>,

    #[clap(
        long,
        help = "Requester private key (hex). Deprecated: register a signing key instead"
    )]
    pub requester_secret: Option<String>,
}

impl JoinGroupContextCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = JoinGroupContextApiRequest {
            context_id: self.context_id,
            joiner_identity: self.joiner_identity,
            requester_secret: self.requester_secret,
        };

        let client = environment.client()?;
        let response = client.join_group_context(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
