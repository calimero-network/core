use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::Parser;
use eyre::{bail, Result};

use super::Capability;
use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Revoke capabilities from a member (use 'meroctl group members set-caps' for group-level capabilities)")]
pub struct RevokePermissionCommand {
    #[arg(help = "The context ID")]
    #[arg(long, short, default_value = "default")]
    pub context: Alias<ContextId>,

    #[arg(help = "The revoker's public key")]
    #[arg(long = "as", default_value = "default")]
    pub revoker: Alias<PublicKey>,

    #[arg(help = "The revokee's public key")]
    pub revokee: Alias<PublicKey>,

    #[arg(help = "The capability to revoke")]
    #[clap(value_enum)]
    pub capability: Capability,
}

impl RevokePermissionCommand {
    pub async fn run(self, _environment: &mut Environment) -> Result<()> {
        bail!(
            "Per-context capability revoke has been removed.\n\
             Use group-level capabilities instead:\n\
             \n\
             meroctl group members set-caps <group_id> <identity> <capabilities>"
        )
    }
}
