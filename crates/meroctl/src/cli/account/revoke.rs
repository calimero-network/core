use calimero_server_primitives::admin::RevokeDeviceApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

/// Withdraw a device from an account, terminally.
///
/// The device id is spent for good — re-enrolling that machine mints a fresh
/// one. That permanence is deliberate: it keeps a replica id from ever being
/// reused, so the CRDT collections hold their one-writer-per-replica invariant
/// across a revoke and re-add.
///
/// Run it as a group **admin** to revoke any device; the scope key rotates in
/// the same op, so the device loses both writing and reading.
///
/// Run it on the node holding the **account** to disown your own lost machine
/// without an admin — it attaches a root-signed proof. That path cannot rotate
/// the key (only an admin may), so the device stops writing at once but can
/// still read until an admin rotates. The command reports which happened.
#[derive(Clone, Debug, Parser)]
#[command(about = "Withdraw a device from an account")]
pub struct RevokeCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(
        long,
        value_name = "HEX",
        help = "The device ID to revoke, 64 hex chars"
    )]
    pub device_id: String,
}

impl RevokeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client
            .revoke_device(
                &self.namespace_id,
                RevokeDeviceApiRequest {
                    device_id: self.device_id,
                },
            )
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
