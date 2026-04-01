use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::Parser;
use eyre::{bail, Result};

use super::Capability;
use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Grant capabilities to a member (use 'meroctl group members set-caps' for group-level capabilities)")]
pub struct GrantPermissionCommand {
    #[arg(help = "The context ID")]
    #[arg(long, short, default_value = "default")]
    pub context: Alias<ContextId>,

    #[arg(help = "The granter's public key")]
    #[arg(long = "as", default_value = "default")]
    pub granter: Alias<PublicKey>,

    #[arg(help = "The grantee's public key")]
    pub grantee: Alias<PublicKey>,

    #[arg(help = "The capability to grant")]
    #[clap(value_enum)]
    pub capability: Capability,
}

impl GrantPermissionCommand {
    pub async fn run(self, _environment: &mut Environment) -> Result<()> {
        bail!(
            "Per-context capability grant has been removed.\n\
             Use group-level capabilities instead:\n\
             \n\
             meroctl group members set-caps <group_id> <identity> <capabilities>"
        )
    }
}
