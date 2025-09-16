use calimero_primitives::identity::PublicKey;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Remove an identity", aliases = ["rm", "del", "delete"])]
pub struct RemoveCommand {
    #[arg(help = "Public key of the identity to remove")]
    pub public_key: PublicKey,
}

impl RemoveCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.remove_identity(&self.public_key).await?;
        environment.output.write(&response);
        Ok(())
    }
}
