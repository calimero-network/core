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
///
/// Or pass `--proof` with a proof minted offline by `merod account revoke-proof`.
/// That is the case where the account root is not on any node — it lives on paper
/// — and the machine that held the device is gone. The node this runs against
/// needs no authority of its own; it only publishes.
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

    /// A revocation proof from `merod account revoke-proof`, or `@PATH` to read
    /// one from a file.
    ///
    /// `@PATH` exists because the blob is long enough that a shell history entry
    /// is an awkward place for it, not because it is secret — it authorises this
    /// one revocation and nothing else.
    #[clap(long, value_name = "HEX|@PATH")]
    pub proof: Option<String>,
}

impl RevokeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let proof = match self.proof {
            // Trimmed because a file written by `revoke-proof > file` ends in a
            // newline, and hex with a trailing newline is not hex.
            Some(raw) => Some(match raw.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)
                    .map_err(|err| eyre::eyre!("failed to read the proof from {path}: {err}"))?
                    .trim()
                    .to_owned(),
                None => raw.trim().to_owned(),
            }),
            None => None,
        };

        let client = environment.client()?;
        let response = client
            .revoke_device(
                &self.namespace_id,
                RevokeDeviceApiRequest {
                    device_id: self.device_id,
                    proof,
                },
            )
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
