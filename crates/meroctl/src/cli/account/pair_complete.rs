use calimero_server_primitives::admin::PairDeviceCompleteApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

/// Certify a device another node minted, link it, and hand it the scope key.
///
/// The second half of pairing, run on the device that already **holds** the
/// account — it is the only one with the account root that can sign the
/// certificate. The values come from `account pair-init` on the new device.
///
/// The statement is what makes them more than claims by whoever relayed them:
/// it is the new device's signature over its own id and both keys, and this
/// refuses to certify without it.
///
/// `--confirmation-code` is the other half, and it is required rather than
/// advisory: get it from whoever is at the pairing device — read out, not
/// pasted along with the keys — and this refuses to certify if it does not
/// describe the key material that arrived. An attacker that replaced both keys
/// can re-sign the statement; it cannot produce that code.
///
/// This publishes two ops: the link, which confers authority, and a key
/// delivery, without which the new device is linked but still cannot read
/// anything.
///
/// Only the *current* scope key is delivered. The paired device converges on
/// forward state; it cannot decrypt ops sealed under retired key epochs.
#[derive(Clone, Debug, Parser)]
#[command(about = "Certify and link a device minted by `account pair-init`")]
pub struct PairCompleteCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(
        long,
        value_name = "HEX",
        help = "The device ID printed by pair-init, 64 hex chars"
    )]
    pub device_id: String,

    #[clap(
        long,
        value_name = "HEX",
        help = "The device KEM key printed by pair-init, 64 hex chars"
    )]
    pub kem_key: String,

    #[clap(
        long,
        value_name = "HEX",
        help = "The device signing key printed by pair-init, 64 hex chars"
    )]
    pub sign_key: String,

    #[clap(
        long,
        value_name = "HEX",
        help = "The pairing statement printed by pair-init, 128 hex chars"
    )]
    pub statement: String,

    #[clap(
        long,
        value_name = "CODE",
        help = "The confirmation code read out from the pairing device, e.g. 7BC0-DAAC-CCB4-84A4"
    )]
    pub confirmation_code: String,
}

impl PairCompleteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client
            .pair_device_complete(
                &self.namespace_id,
                PairDeviceCompleteApiRequest {
                    device_id: self.device_id,
                    kem_public_key: self.kem_key,
                    sign_public_key: self.sign_key,
                    statement: self.statement,
                    confirmation_code: self.confirmation_code,
                },
            )
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
