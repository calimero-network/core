use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

/// Enroll this node's device into a namespace under a fresh account.
///
/// Run this AFTER the node has joined the namespace and received its scope key.
/// A device link travels as an encrypted group op, so a node holding no key
/// cannot publish one — the node refuses with that reason rather than failing
/// obscurely, but the ordering is worth knowing before you hit it.
#[derive(Clone, Debug, Parser)]
#[command(about = "Enroll this node's device under a fresh account")]
pub struct CreateCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,
}

impl CreateCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.create_account(&self.namespace_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}
