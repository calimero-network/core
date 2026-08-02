use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

/// Report which account this node speaks for in a namespace.
///
/// The read everything else needs first: granting a writer names an account,
/// revoking names an account, and an account id is *derived* from this node's
/// root rather than carried on the wire — so without this there is no way to
/// learn one except by keeping the output of `account create`.
///
/// Always answerable, even before `account create`: the id exists as soon as the
/// node has a root. What may be missing is the device, and a missing one is the
/// signal that this node has not enrolled here yet.
#[derive(Clone, Debug, Parser)]
#[command(about = "Show this node's account for a namespace")]
pub struct ShowCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,
}

impl ShowCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.get_namespace_account(&self.namespace_id).await?;
        environment.output.write(&response);
        Ok(())
    }
}
