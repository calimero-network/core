use calimero_server_primitives::admin::PairDeviceInitApiRequest;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

/// Mint a device on this node for an account that already exists elsewhere.
///
/// The first half of pairing, run on the **new** device. Pairing has to be a
/// two-way exchange: this node cannot mint its device id until it knows the
/// account, and the account holder cannot certify that device until it knows the
/// id and agreement key. So this command produces those two values and stops.
///
/// Hand every printed value to the device that already holds the account and run
/// `account pair-complete` there. The statement travels with them — that side
/// refuses to certify key material that does not come with the signature of the
/// device that minted it. Then read the confirmation code out to whoever is
/// running `pair-complete`: if it does not match theirs, the payload was altered
/// in transit and the device must not be certified.
///
/// The root key and nonce come from that device — `account create` prints both.
/// Neither is a secret: a genesis is public data, and naming an account you do
/// not hold gains you nothing, because the device stays inert until the account
/// root signs its certificate.
///
/// Unlike `account create`, this needs no scope key and no membership. That is
/// the point — a paired device is a device of somebody else's account and a
/// member of nothing.
#[derive(Clone, Debug, Parser)]
#[command(about = "Mint a device on this node for an existing account")]
pub struct PairInitCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(
        long,
        value_name = "HEX",
        help = "The account's epoch-0 root key, 64 hex chars"
    )]
    pub root_key: String,

    #[clap(
        long,
        value_name = "HEX",
        help = "The account's genesis nonce, 32 hex chars"
    )]
    pub nonce: String,
}

impl PairInitCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client
            .pair_device_init(
                &self.namespace_id,
                PairDeviceInitApiRequest {
                    account_root_key: self.root_key,
                    account_nonce: self.nonce,
                },
            )
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
