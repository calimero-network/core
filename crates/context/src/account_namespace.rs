//! Publishing into this node's account namespace: one device's row into its
//! registry, and one namespace into its set.
//!
//! One publisher per op. The registry's scope epoch is minted from THIS node's
//! folded row and only the account holder ever signs, so a single publisher is
//! what keeps two statements off the same epoch; `announce`'s guard - a node
//! with no account namespace of its own has nowhere to write, and it is never
//! announced into itself - is the same at all four of its callers.

use calimero_account::{AccountProof, DeviceCert, DeviceScope};
use calimero_context_client::local_governance::{AckRouter, GroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{
    AccountDeviceRegistry, AccountNamespaceSet, AccountRoot, MetaRepository, NamespaceRepository,
    NodeDeviceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
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

/// What this node did to a namespace, from the account's point of view.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AccountNamespaceChange {
    Gained,
    Left,
}

/// Tell this account's other devices that this node gained or left `namespace`.
///
/// Best effort by design: a device that misses it re-reads the set from the DAG,
/// and no creation, join or leave may fail because a publish did not. Mirrors
/// `publish_device_certified`'s shape: `true` only once the local apply has
/// recorded the set row, `false` on any warned failure.
pub(crate) async fn announce(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: ContextGroupId,
    change: AccountNamespaceChange,
) -> bool {
    match publish(store, node_client, ack_router, namespace, change).await {
        Ok(recorded) => recorded,
        Err(err) => {
            warn!(
                ?err,
                ?namespace,
                ?change,
                "failed to tell this account's other devices about a namespace"
            );
            false
        }
    }
}

async fn publish(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: ContextGroupId,
    change: AccountNamespaceChange,
) -> EyreResult<bool> {
    let Some(account_namespace) = NodeDeviceRepository::new(store).account_namespace()? else {
        return Ok(false);
    };
    if account_namespace == namespace {
        return Ok(false);
    }
    // Participation, not the row: the holder names its account namespace before
    // creating it, and this is what says there is a DAG to write to.
    let Some((_signer_pk, signer_sk)) =
        NamespaceRepository::new(store).identity(&account_namespace)?
    else {
        return Ok(false);
    };

    let (op, expected) = match change {
        AccountNamespaceChange::Gained => {
            let application = target_application(store, namespace)?;
            (
                GroupOp::AccountNamespaceGained {
                    namespace,
                    application,
                },
                Some(application),
            )
        }
        AccountNamespaceChange::Left => (GroupOp::AccountNamespaceLeft { namespace }, None),
    };
    let op_kind = op.op_kind_label();

    calimero_governance_store::sign_apply_and_publish(
        store,
        node_client,
        ack_router,
        &account_namespace,
        &PrivateKey::from(signer_sk),
        op,
    )
    .await?
    .observe("account_namespace", op_kind);

    // The op's own local apply is the only durable write, and an apply that
    // refuses the statement warns rather than failing - so the set is what says it.
    let recorded =
        AccountNamespaceSet::new(store, account_namespace).contains(namespace)? == expected;
    if !recorded {
        warn!(
            ?namespace,
            ?change,
            "the account namespace did not take the change"
        );
    }
    Ok(recorded)
}

/// The application `namespace` targets, as this node has folded it so far.
///
/// The zero id a cold-start seed writes is reported as absent, so no device
/// reads it as a real id it could be scoped to. `device_link::plan` leaves the
/// same zero unfiltered; `KnownDeviceCert::covers` answers the same either way.
fn target_application(
    store: &Store,
    namespace: ContextGroupId,
) -> EyreResult<Option<ApplicationId>> {
    Ok(MetaRepository::new(store)
        .load(&namespace)?
        .map(|meta| meta.target.application_id)
        .filter(|application| *application.as_ref() != [0u8; 32]))
}
