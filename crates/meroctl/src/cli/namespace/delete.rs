use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::DeleteNamespaceApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Delete a namespace")]
pub struct DeleteCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(
        long,
        help = "Public key of the requester (namespace admin). Auto-resolved if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl DeleteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = DeleteNamespaceApiRequest {
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.delete_namespace(&self.namespace_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}
