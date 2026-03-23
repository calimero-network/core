use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::SyncGroupApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Sync a group from its contract state")]
pub struct SyncCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        long,
        help = "Public key of the requester. Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,

    #[clap(long, help = "Optional protocol identifier")]
    pub protocol: Option<String>,

    #[clap(long, help = "Optional network/chain ID")]
    pub network_id: Option<String>,

    #[clap(long, help = "Optional contract ID")]
    pub contract_id: Option<String>,
}

impl SyncCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = SyncGroupApiRequest {
            requester: self.requester,
            protocol: self.protocol,
            network_id: self.network_id,
            contract_id: self.contract_id,
        };

        let client = environment.client()?;
        let response = client.sync_group(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
