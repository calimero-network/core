use calimero_server_primitives::admin::AccountPairInitApiRequest;
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
/// `account pair-complete` there. The statement travels with them - that side
/// refuses to certify key material that does not come with the signature of the
/// device that minted it. Then read the confirmation code out to whoever is
/// running `pair-complete`: if it does not match theirs, the payload was altered
/// in transit and the device must not be certified.
///
/// The root key comes from that device - `meroctl account show` reports it. It is not a
/// secret: a genesis is public data, and naming an account you do not hold gains
/// you nothing, because the device stays inert until the account root signs its
/// certificate.
///
/// Unlike the enrolment a join performs, this needs no scope key and no
/// membership. That is the point - a paired device is a device of somebody
/// else's account and a member of nothing.
///
/// Name the namespaces this device should listen on, or the account namespace alone.
/// One device is minted for the whole set - one id, one key pair and one code to
/// read out - because the certificate covers the account rather than a scope.
/// Either the account namespace id or the namespace set has to be given: a
/// device that is a member of nothing can neither read its account's
/// namespaces off a DAG nor derive them, so only the device that holds the
/// account knows them.
/// Naming the account namespace alone is enough - the device records it and
/// follows it like one more namespace.
#[derive(Clone, Debug, Parser)]
#[command(about = "Mint a device on this node for an existing account")]
pub struct PairInitCommand {
    #[clap(
        name = "NAMESPACE_ID",
        num_args = 0..,
        required_unless_present = "account_namespace",
        help = "The hex-encoded namespace IDs this device will listen on"
    )]
    pub namespace_ids: Vec<String>,

    #[clap(
        long,
        value_name = "HEX",
        help = "The account's epoch-0 root key, 64 hex chars"
    )]
    pub root_key: String,

    #[clap(
        long,
        value_name = "HEX",
        help = "The account namespace id the holder reports on `account show`; enough on its own"
    )]
    pub account_namespace: Option<String>,
}

impl PairInitCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client
            .pair_device_init(AccountPairInitApiRequest {
                account_root_public_key: self.root_key,
                namespaces: self.namespace_ids,
                account_namespace: self.account_namespace,
            })
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
