//! Publishing into this node's account namespace: one device's row into its
//! registry, and one namespace into its set.
//!
//! One publisher per op. The registry's scope epoch is minted from THIS node's
//! folded row and only the account holder ever signs, so a single publisher is
//! what keeps two statements off the same epoch; `announce`'s guard - a node
//! with no account namespace of its own has nowhere to write, and it is never
//! announced into itself - is the same at all four of its callers.

use std::sync::Arc;
use std::time::Duration;

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
use tokio::time::sleep;
use tracing::{debug, warn};

#[cfg(not(test))]
const TARGET_WAIT_INTERVAL: Duration = Duration::from_secs(1); // how often a target-less gain re-reads the namespace's meta
#[cfg(test)]
const TARGET_WAIT_INTERVAL: Duration = Duration::from_millis(20); // the same wait, shortened so tests do not sit out the real one
const TARGET_WAIT_ATTEMPTS: u32 = 30; // how many of those before the gain is announced with no target

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
/// and no creation, join or leave may fail because a publish did not. Nothing to
/// read back - a skip, a failure and a set that did not take the change are all
/// warned here, and a gain with no target yet publishes after this has returned.
/// `site` names the caller on the delivery metric.
pub(crate) async fn announce(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &Arc<AckRouter>,
    namespace: ContextGroupId,
    change: AccountNamespaceChange,
    site: &'static str,
) {
    if let AccountNamespaceChange::Gained = change {
        // A namespace's meta can fold well after the bind that gained it, and no
        // event says when, so a gain with no target waits for one off the caller.
        if matches!(target_application(store, namespace), Ok(None)) {
            wait_for_target_then_announce(store, node_client, ack_router, namespace, site);
            return;
        }
    }
    publish_or_warn(store, node_client, ack_router, namespace, change, site).await;
}

/// Announce the gain of `namespace` once its target application is known, or
/// with none once [`TARGET_WAIT_ATTEMPTS`] have passed. Detached; never awaited.
fn wait_for_target_then_announce(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &Arc<AckRouter>,
    namespace: ContextGroupId,
    site: &'static str,
) {
    let store = store.clone();
    let node_client = node_client.clone();
    let ack_router = Arc::clone(ack_router);
    drop(tokio::spawn(async move {
        for _ in 0..TARGET_WAIT_ATTEMPTS {
            sleep(TARGET_WAIT_INTERVAL).await;
            if !matches!(target_application(&store, namespace), Ok(None)) {
                break;
            }
        }
        if matches!(target_application(&store, namespace), Ok(None)) {
            debug!(
                ?namespace,
                "no target application after waiting; announcing the gain with none"
            );
        }
        publish_or_warn(
            &store,
            &node_client,
            &ack_router,
            namespace,
            AccountNamespaceChange::Gained,
            site,
        )
        .await;
    }));
}

async fn publish_or_warn(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: ContextGroupId,
    change: AccountNamespaceChange,
    site: &'static str,
) {
    if let Err(err) = publish(store, node_client, ack_router, namespace, change, site).await {
        warn!(
            ?err,
            ?namespace,
            ?change,
            "failed to tell this account's other devices about a namespace"
        );
    }
}

async fn publish(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    namespace: ContextGroupId,
    change: AccountNamespaceChange,
    site: &'static str,
) -> EyreResult<()> {
    let Some(account_namespace) = NodeDeviceRepository::new(store).account_namespace()? else {
        return Ok(());
    };
    if account_namespace == namespace {
        return Ok(());
    }
    // Participation, not the row: the holder names its account namespace before
    // creating it, and this is what says there is a DAG to write to.
    let Some((_signer_pk, signer_sk)) =
        NamespaceRepository::new(store).identity(&account_namespace)?
    else {
        return Ok(());
    };

    let op = match change {
        AccountNamespaceChange::Gained => GroupOp::AccountNamespaceGained {
            namespace,
            application: target_application(store, namespace)?,
        },
        AccountNamespaceChange::Left => GroupOp::AccountNamespaceLeft { namespace },
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
    .observe(site, op_kind);

    // The op's own local apply is the only durable write, and an apply that
    // refuses the statement warns rather than failing - so the set is what says it.
    let named = AccountNamespaceSet::new(store, account_namespace)
        .contains(namespace)?
        .is_some();
    if named != matches!(change, AccountNamespaceChange::Gained) {
        warn!(
            ?namespace,
            ?change,
            "the account namespace did not take the change"
        );
    }
    Ok(())
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
