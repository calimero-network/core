use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, Parser)]
#[command(about = "Get this node's identity for a namespace")]
pub struct IdentityCommand {
    /// Namespace ID (hex-encoded root group id)
    pub namespace_id: String,
}

impl IdentityCommand {
    pub async fn run(self, _environment: &mut Environment) -> Result<()> {
        let client = _environment.client()?;
        let response = client
            .get_namespace_identity(&self.namespace_id)
            .await?;

        println!("{}", serde_json::to_string_pretty(&response)?);

        Ok(())
    }
}
