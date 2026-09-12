//! Publishing one device's row into the account namespace's device registry.
//!
//! One place rather than one per publisher, because the scope epoch is minted
//! from THIS node's folded row and only the account holder ever signs, so a
//! single publisher is what keeps two statements off the same epoch.

use calimero_account::{AccountProof, DeviceCert, DeviceScope};
use calimero_context_client::local_governance::{AckRouter, GroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{AccountDeviceRegistry, AccountRoot};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use tracing::warn;

/// Record `certificate` and `applications` in `namespace`'s registry, at the
/// next scope epoch for that device.
///
/// `true` only when the registry took the statement. Never fails the caller:
/// every step here runs after the work a request was made for is already done,
/// so a failure is a `warn!` and the next pairing or relink republishes it.
/// `site` names the handler on the delivery metric.
#[allow(clippy::too_many_arguments, reason = "orthogonal publish-path args")]
pub async fn publish_device_certified(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: ContextGroupId,
    signer_sk: &PrivateKey,
    root: &AccountRoot,
    certificate: &AccountProof<DeviceCert>,
    applications: &[ApplicationId],
    site: &'static str,
) -> bool {
    let device = certificate.statement.device;
    let registry = AccountDeviceRegistry::new(store, namespace);
    let scope_epoch = match registry.device(device) {
        Ok(stored) => stored.map_or(0, |(_cert, epoch)| epoch.saturating_add(1)),
        Err(err) => {
            warn!(%device, %err, "could not read the device's registry row, so it was not recorded");
            return false;
        }
    };

    // Key epoch 0: the account root has not rotated (rotation is not implemented
    // yet), so the certifying key is the genesis key and there are no handoffs.
    let statement = match DeviceScope::sign(
        root.signing_key(),
        certificate.statement.account,
        device,
        applications.to_vec(),
        scope_epoch,
        0,
    ) {
        Ok(statement) => statement,
        Err(err) => {
            warn!(%device, %err, "could not sign the device's scope, so it was not recorded");
            return false;
        }
    };

    // Both anchors taken from the certificate, so the two proofs in one op can
    // never resolve the root key at different epochs.
    let scope = AccountProof {
        genesis: certificate.genesis,
        chain: certificate.chain.clone(),
        statement,
    };
    match calimero_governance_store::sign_apply_and_publish(
        store,
        node_client,
        ack_router,
        &namespace,
        signer_sk,
        GroupOp::AccountDeviceCertified {
            certificate: Box::new(certificate.clone()),
            scope: Box::new(scope),
        },
    )
    .await
    {
        Ok(report) => report.observe(site, "AccountDeviceCertified"),
        Err(err) => {
            warn!(%device, %err, "the device was not recorded in the account namespace");
            return false;
        }
    }

    // The op's own local apply is the only durable write, and an apply that
    // refuses the statement warns rather than failing - so the row is what says it.
    match registry.device(device) {
        Ok(Some((_cert, epoch))) if epoch == scope_epoch => true,
        Ok(_) => {
            warn!(%device, "the account namespace did not take the device's scope");
            false
        }
        Err(err) => {
            warn!(%device, %err, "could not confirm the device's registry row");
            false
        }
    }
}
