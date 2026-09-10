//! Following what this account gains, unfollowing what it leaves, projecting a
//! sibling it certifies, and carrying a revocation it proved.
//!
//! The device side of the account namespace: a listener on the op-event channel,
//! beside [`crate::auto_follow`] and shaped like it. Four events, and each is
//! acted on only where the guard says it arrived on THIS node's account
//! namespace - the same op on a project's DAG is another account's business, or
//! this node's own publish coming back.
//!
//! Projecting is carrying a newly certified sibling into the namespaces this node
//! takes part in whose target its scope covers. Every online device folds that one
//! op, so k of them would publish the same m binds; a random wait of up to
//! [`PROJECTION_JITTER_MAX_MS`] first lets a faster sibling's link land, which the
//! per-namespace already-bound read inside the bind then skips.
//!
//! Carrying is republishing a proof-bearing revocation folded here into the
//! namespaces this node takes part in where the device is still linked. The
//! proof names an account and a device and no namespace, so it verifies wherever
//! it lands, and a device the revoker could not reach is withdrawn there too.
//!
//! What projection does not repair: a sibling whose certificate this node folded
//! BEFORE it took part in N is never carried into N by this node, because
//! following N binds nobody. A relink is what repairs that.
//!
//! Following is [`crate::handlers::follow_namespace`]: note participation,
//! subscribe, pull. From there the existing machinery converges the namespace - the
//! beacon rescue pulls its DAG, the key pull succeeds because the gainer bound
//! this device, the target folds, bytecode acquisition runs and contexts join
//! through the account's auto-follow flags. Both halves are idempotent, so
//! following one this node already takes part in costs nothing.
//!
//! Unfollowing pulls the namespace once and then unsubscribes: the local view of
//! it turns on folding its own `MemberLeft`, which never arrives on a dropped
//! topic. Local state is kept, as the holder's own `leave_namespace` keeps it, so
//! a later gain re-follows into state that is already there.
//!
//! Every start sweeps both ways, because nothing re-drives an op applied while no
//! listener was up. The unfollow half is deliberately conservative: this node has
//! to have folded the namespace, the set has to have dropped it, AND its member
//! row has to show the account gone. An unsynced namespace answers "absent" to
//! the last two, and dropping it on that alone would unfollow what it is still in.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use calimero_account::{AccountId, DeviceId, SignedDeviceRevocation};
use calimero_context_client::client::ContextClient;
use calimero_context_client::group::FollowNamespaceRequest;
use calimero_context_client::local_governance::{AckRouter, GroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_governance_store::{
    bind_device_everywhere, revoke_device_in, AccountBindingRepository, AccountDeviceRegistry,
    AccountNamespaceSet, KnownDeviceCert, MembershipRepository, NamespaceDagService,
    NamespaceRepository, NodeDeviceRepository,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::handlers::pair_device_complete::{namespaces_in_scope, signing_identity};

#[cfg(not(test))]
const PROJECTION_JITTER_MAX_MS: u64 = 2_000; // damps k devices publishing one certification's m binds at once
#[cfg(test)]
const PROJECTION_JITTER_MAX_MS: u64 = 5; // the tests assert on the bind, not on the wait

struct HandleState {
    abort: AbortHandle,
}

static HANDLE: Mutex<Option<HandleState>> = Mutex::new(None);

/// Spawn the account-follow handler. Returns immediately; the handler runs as a
/// detached tokio task for the process lifetime.
///
/// Subscribes synchronously before spawning, for the reason `auto_follow::spawn`
/// gives: an event fired between this returning and the task's first poll would
/// otherwise be lost, and the DAG re-drives it only on the next restart.
pub fn spawn(
    store: Store,
    node_client: NodeClient,
    ack_router: Arc<AckRouter>,
    context_client: ContextClient,
) {
    let mut slot = HANDLE.lock().expect("account-follow HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.abort.is_finished()) {
        debug!("account-follow handler already running; skipping re-spawn");
        return;
    }
    let rx = op_events::subscribe();
    let abort = tokio::spawn(async move {
        run(rx, store, node_client, ack_router, context_client).await;
    })
    .abort_handle();
    *slot = Some(HandleState { abort });
}

/// Abort the running handler task. Safe with none running; after it, [`spawn`]
/// may be called again to rebind to a new store or clients. Aborting drops the
/// run task's `JoinSet`, so the follows still in flight are aborted with it.
pub fn shutdown() {
    if let Some(state) = HANDLE
        .lock()
        .expect("account-follow HANDLE poisoned")
        .take()
    {
        state.abort.abort();
    }
}

async fn run(
    mut rx: broadcast::Receiver<OpEvent>,
    store: Store,
    node_client: NodeClient,
    ack_router: Arc<AckRouter>,
    context_client: ContextClient,
) {
    info!("account-follow handler started");

    // Nothing re-drives a gain or a leave applied while no listener was up - a
    // lagged recv, or a restart's shutdown-to-spawn window - so a start sweeps both.
    for namespace in namespaces_the_account_left(&store) {
        unfollow(&node_client, namespace).await;
    }
    if let Some((account_namespace, own)) = own_registry_scope(&store) {
        for namespace in namespaces_now_covered(&store, account_namespace.to_bytes(), own.device())
        {
            follow(&context_client, namespace).await;
        }
    }

    // Per-event work runs on its own task and this set owns the handles, so
    // aborting the handler aborts the follows still in flight - as `auto_follow`.
    let mut tasks = tokio::task::JoinSet::new();

    loop {
        // Reap finished handlers so the set does not grow across a long run.
        while tasks.try_join_next().is_some() {}

        let event = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    skipped,
                    "account-follow subscriber lagged; a gain dropped here is followed by \
                     the sweep on this node's next start"
                );
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                info!("account-follow op-event channel closed; handler exiting");
                break;
            }
        };

        // Each event on its own task: a follow waits on an actor round-trip and a
        // subscribe, and a blocked recv loop drops unrelated events as `Lagged`.
        match event {
            OpEvent::AccountNamespaceGained {
                group_id,
                namespace,
                application,
            } => {
                let store = store.clone();
                let context_client = context_client.clone();
                let _ = tasks.spawn(async move {
                    handle_namespace_gained(
                        &store,
                        &context_client,
                        group_id,
                        namespace,
                        application,
                    )
                    .await;
                });
            }
            OpEvent::AccountNamespaceLeft {
                group_id,
                namespace,
            } => {
                let store = store.clone();
                let node_client = node_client.clone();
                let _ = tasks.spawn(async move {
                    handle_namespace_left(&store, &node_client, group_id, namespace).await;
                });
            }
            OpEvent::AccountDeviceCertified { group_id, device } => {
                // Two rules on one event, and they are exclusive by device: this
                // node's own scope arriving is a follow, any other device's is a bind.
                let carry_store = store.clone();
                let carry_node_client = node_client.clone();
                let carry_ack_router = Arc::clone(&ack_router);
                let _ = tasks.spawn(async move {
                    handle_sibling_certified(
                        &carry_store,
                        &carry_node_client,
                        &carry_ack_router,
                        group_id,
                        device,
                    )
                    .await;
                });
                let store = store.clone();
                let context_client = context_client.clone();
                let _ = tasks.spawn(async move {
                    handle_device_certified(&store, &context_client, group_id, device).await;
                });
            }
            OpEvent::DeviceRevoked {
                group_id,
                account,
                device,
                proof: Some(proof),
            } => {
                let store = store.clone();
                let node_client = node_client.clone();
                let ack_router = Arc::clone(&ack_router);
                let _ = tasks.spawn(async move {
                    handle_revocation_carry(
                        &store,
                        &node_client,
                        &ack_router,
                        group_id,
                        account,
                        device,
                        *proof,
                    )
                    .await;
                });
            }
            _ => {}
        }
    }
}

/// This node's own row in its account namespace's registry.
///
/// `None` on a node that follows no account namespace, and on one whose own
/// certified op has not been folded here yet - which simply does nothing until
/// it arrives.
fn own_registry_scope(store: &Store) -> Option<(ContextGroupId, KnownDeviceCert)> {
    let devices = NodeDeviceRepository::new(store);
    let resolved = || -> EyreResult<Option<(ContextGroupId, KnownDeviceCert)>> {
        let Some(account_namespace) = devices.account_namespace()? else {
            return Ok(None);
        };
        let Some(held) = devices.get()? else {
            return Ok(None);
        };
        Ok(AccountDeviceRegistry::new(store, account_namespace)
            .device(held.device())?
            .map(|(cert, _epoch)| (account_namespace, cert)))
    };
    match resolved() {
        Ok(found) => found,
        Err(err) => {
            warn!(?err, "account-follow: failed to read this node's own scope");
            None
        }
    }
}

/// This node's account namespace, when `group_id` is it.
///
/// Every rule in this module is about facts the ACCOUNT wrote down; the same op
/// folded on a project's DAG is another account's business, or this node's own
/// publish coming back.
fn account_namespace_if_ours(store: &Store, group_id: [u8; 32]) -> Option<ContextGroupId> {
    match NodeDeviceRepository::new(store).account_namespace() {
        Ok(Some(account_namespace)) if account_namespace.to_bytes() == group_id => {
            Some(account_namespace)
        }
        Ok(_) => None,
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read this node's account namespace"
            );
            None
        }
    }
}

/// Has this node folded `namespace`'s governance at all? Without a head, an
/// absent set row and an absent member row are silence rather than a verdict.
fn folded_here(store: &Store, namespace: ContextGroupId) -> EyreResult<bool> {
    Ok(
        !NamespaceDagService::new(store, namespace.to_bytes().into())
            .read_head_record()?
            .parent_hashes
            .is_empty(),
    )
}

/// The namespaces this node takes part in that its account has left.
///
/// Three reads, and all three have to say so: an unsynced namespace and an
/// unsynced set both read as absent, and unfollowing on that would cut this
/// node off a namespace it is still in.
fn namespaces_the_account_left(store: &Store) -> Vec<ContextGroupId> {
    let devices = NodeDeviceRepository::new(store);
    let resolved = || -> EyreResult<Vec<ContextGroupId>> {
        let (Some(account_namespace), Some(held)) = (devices.account_namespace()?, devices.get()?)
        else {
            return Ok(Vec::new());
        };
        let set = AccountNamespaceSet::new(store, account_namespace);
        let members = MembershipRepository::new(store);
        let mut left = Vec::new();
        for namespace in NamespaceRepository::new(store).participating_namespaces()? {
            if namespace == account_namespace {
                continue;
            }
            let dropped = || -> EyreResult<bool> {
                Ok(folded_here(store, namespace)?
                    && set.contains(namespace)?.is_none()
                    && !members.is_member(&namespace, &held.account)?)
            };
            match dropped() {
                Ok(true) => left.push(namespace),
                Ok(false) => {}
                // Skipped, not fatal: a namespace whose reads fail is one this
                // sweep knows nothing about, and the next start asks again.
                Err(err) => warn!(
                    ?err,
                    ?namespace,
                    "account-follow: failed to decide whether the account left a namespace"
                ),
            }
        }
        Ok(left)
    };
    match resolved() {
        Ok(left) => left,
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read what this account has left"
            );
            Vec::new()
        }
    }
}

/// Does a gain announced in `group_id`, of a namespace targeting `application`,
/// ask this node to follow it?
pub(crate) fn follows_on_gain(
    store: &Store,
    group_id: [u8; 32],
    application: Option<ApplicationId>,
) -> bool {
    match own_registry_scope(store) {
        Some((account_namespace, own)) => {
            account_namespace.to_bytes() == group_id && own.covers(application)
        }
        None => false,
    }
}

/// The namespaces a freshly certified scope for THIS node's device now covers.
///
/// How a device paired after the account gained its namespaces, or widened
/// later, follows what it now may. Empty for any other device or group.
pub(crate) fn namespaces_now_covered(
    store: &Store,
    group_id: [u8; 32],
    device: DeviceId,
) -> Vec<ContextGroupId> {
    let Some((account_namespace, own)) = own_registry_scope(store) else {
        return Vec::new();
    };
    if account_namespace.to_bytes() != group_id || own.device() != device {
        return Vec::new();
    }
    match AccountNamespaceSet::new(store, account_namespace).namespaces() {
        Ok(set) => set
            .into_iter()
            .filter(|(_, application)| own.covers(*application))
            .map(|(namespace, _)| namespace)
            .collect(),
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read the account's namespace set"
            );
            Vec::new()
        }
    }
}

/// The certificate to publish for a newly certified device of this account, and
/// the namespaces this node has to publish it into.
///
/// `None` for this node's own device - the holder that signed it published it
/// already - and never the account namespace, whose unset target no scope could
/// name. Also `None` for a device revoked ANYWHERE this node takes part: the id
/// is spent everywhere, and a revoker publishes its tombstone only where it takes
/// part, so the target's own DAG is no gate.
pub(crate) fn namespaces_to_bind_into(
    store: &Store,
    group_id: [u8; 32],
    device: DeviceId,
) -> Option<(KnownDeviceCert, Vec<ContextGroupId>)> {
    let account_namespace = account_namespace_if_ours(store, group_id)?;
    let devices = NodeDeviceRepository::new(store);
    match devices.get() {
        Ok(Some(held)) if held.device() == device => return None,
        Ok(_) => {}
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to read this node's own device");
            return None;
        }
    }
    match devices.revoked_in(device) {
        Ok(revoked) if !revoked.is_empty() => return None,
        Ok(_) => {}
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to read where a sibling is revoked");
            return None;
        }
    }
    let cert = match AccountDeviceRegistry::new(store, account_namespace).device(device) {
        Ok(Some((cert, _epoch))) => cert,
        Ok(None) => return None,
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to read a sibling's certificate");
            return None;
        }
    };
    let targets = match namespaces_in_scope(store, &cert.applications) {
        Ok(targets) => targets,
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to resolve a sibling's scope");
            return None;
        }
    };
    let targets: Vec<_> = targets
        .into_iter()
        .filter(|namespace| *namespace != account_namespace)
        .collect();
    (!targets.is_empty()).then_some((cert, targets))
}

/// The namespaces a proof-bearing revocation folded here has to be carried into.
///
/// An admin revocation authorises nothing outside its own group, so it is not
/// carried. `is_device_linked` reads the bindings still in force, so one that
/// has already tombstoned the device is skipped by the same call - and, unlike
/// the revoker's own `raw_binding` read, one superseded by a root rotation too.
pub(crate) fn namespaces_to_revoke_in(
    store: &Store,
    group_id: [u8; 32],
    device: DeviceId,
    proof: Option<&SignedDeviceRevocation>,
) -> Vec<ContextGroupId> {
    if proof.is_none() {
        return Vec::new();
    }
    let Some(account_namespace) = account_namespace_if_ours(store, group_id) else {
        return Vec::new();
    };
    let namespaces = match NamespaceRepository::new(store).participating_namespaces() {
        Ok(namespaces) => namespaces,
        Err(err) => {
            warn!(
                ?err,
                "account-follow: failed to read the namespaces this node takes part in"
            );
            return Vec::new();
        }
    };
    let bindings = AccountBindingRepository::new(store);
    let mut targets = Vec::new();
    for namespace in namespaces {
        if namespace == account_namespace {
            continue;
        }
        match bindings.is_device_linked(&namespace, device) {
            Ok(true) => targets.push(namespace),
            Ok(false) => {}
            // Skipped, not fatal: the other namespaces still get the carry.
            Err(err) => warn!(?err, ?namespace, %device,
                              "account-follow: failed to read whether a device is still linked"),
        }
    }
    targets
}

/// Does a leave announced in `group_id` ask this node to unfollow?
///
/// Deliberately not gated on a registry row: a device unfollows what its account
/// left whether or not its own scope has arrived. Never the account namespace
/// itself, which is the topic every event acted on here arrives over.
pub(crate) fn unfollows_on_left(
    store: &Store,
    group_id: [u8; 32],
    namespace: ContextGroupId,
) -> bool {
    account_namespace_if_ours(store, group_id)
        .is_some_and(|account_namespace| namespace != account_namespace)
}

async fn handle_namespace_gained(
    store: &Store,
    context_client: &ContextClient,
    group_id: [u8; 32],
    namespace: ContextGroupId,
    application: Option<ApplicationId>,
) {
    if follows_on_gain(store, group_id, application) {
        follow(context_client, namespace).await;
    }
}

async fn handle_device_certified(
    store: &Store,
    context_client: &ContextClient,
    group_id: [u8; 32],
    device: DeviceId,
) {
    for namespace in namespaces_now_covered(store, group_id, device) {
        follow(context_client, namespace).await;
    }
}

/// `Linked { key_delivered: true }` below is published, not accepted: a projector
/// that is no admin there has its `KeyDelivery` refused, and the sibling recovers
/// the key by pulling it, as any participant holding none does.
async fn handle_sibling_certified(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    group_id: [u8; 32],
    device: DeviceId,
) {
    let Some((cert, targets)) = namespaces_to_bind_into(store, group_id, device) else {
        return;
    };
    let signer_sk = match signing_identity(store, &targets) {
        Ok(secret) => PrivateKey::from(secret),
        Err(err) => {
            warn!(?err, %device, "account-follow: no identity to publish a sibling's link with");
            return;
        }
    };
    publish_sibling_link(store, node_client, ack_router, &targets, &signer_sk, &cert).await;
}

/// Wait, then bind - the wait damps k devices publishing one certification's m
/// binds at once, and the per-namespace already-bound read skips what landed.
async fn publish_sibling_link(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    targets: &[ContextGroupId],
    signer_sk: &PrivateKey,
    cert: &KnownDeviceCert,
) {
    let device = cert.device();
    sleep(Duration::from_millis(rand::random_range(
        0..=PROJECTION_JITTER_MAX_MS,
    )))
    .await;
    // The one rule the per-namespace bind does not repeat, and a revocation can
    // have folded during the wait: a spent id belongs in no namespace at all.
    match NodeDeviceRepository::new(store).revoked_in(device) {
        Ok(revoked) if revoked.is_empty() => {}
        Ok(_) => return,
        Err(err) => {
            warn!(?err, %device, "account-follow: failed to re-read where a sibling is revoked");
            return;
        }
    }
    let outcomes =
        bind_device_everywhere(store, node_client, ack_router, targets, signer_sk, cert).await;
    info!(
        %device,
        ?outcomes,
        "account-follow: carried a newly certified device of this account into the \
         namespaces this node takes part in"
    );
}

/// The carry terminates: it publishes only into non-account namespaces, and the
/// `DeviceRevoked` re-emitted there fails `account_namespace_if_ours`.
async fn handle_revocation_carry(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    group_id: [u8; 32],
    account: AccountId,
    device: DeviceId,
    proof: SignedDeviceRevocation,
) {
    let targets = namespaces_to_revoke_in(store, group_id, device, Some(&proof));
    if targets.is_empty() {
        return;
    }
    let signer_sk = match signing_identity(store, &targets) {
        Ok(secret) => PrivateKey::from(secret),
        Err(err) => {
            warn!(?err, %device, "account-follow: no identity to carry a revocation with");
            return;
        }
    };
    let op = GroupOp::AccountDeviceUnlinked {
        account,
        device,
        proof: Some(proof),
    };
    info!(
        %device,
        %account,
        namespaces = targets.len(),
        "account-follow: carrying a revocation into the namespaces the revoker could not reach"
    );
    carry_into(
        store,
        node_client,
        ack_router,
        &targets,
        &signer_sk,
        device,
        &op,
    )
    .await;
}

/// Publish `op` into each target, skipping any the device has left since the
/// decision above was taken: a faster sibling of this account withdrew it there.
async fn carry_into(
    store: &Store,
    node_client: &NodeClient,
    ack_router: &AckRouter,
    targets: &[ContextGroupId],
    signer_sk: &PrivateKey,
    device: DeviceId,
    op: &GroupOp,
) {
    let bindings = AccountBindingRepository::new(store);
    for namespace in targets {
        match bindings.is_device_linked(namespace, device) {
            Ok(true) => {}
            Ok(false) => continue,
            // A read fault costs a duplicate publish at worst, so carry anyway.
            Err(err) => warn!(?err, ?namespace, %device,
                              "account-follow: failed to re-read a carried device's binding"),
        }
        if let Err(err) = revoke_device_in(
            store,
            node_client,
            ack_router,
            namespace,
            signer_sk,
            device,
            op.clone(),
        )
        .await
        {
            warn!(
                ?err,
                ?namespace,
                %device,
                "account-follow: a namespace did not take the carried revocation; the rest continue"
            );
        }
    }
}

async fn handle_namespace_left(
    store: &Store,
    node_client: &NodeClient,
    group_id: [u8; 32],
    namespace: ContextGroupId,
) {
    if !unfollows_on_left(store, group_id, namespace) {
        return;
    }
    // Pull it once before dropping the topic: this node's view of the namespace
    // turns on folding its `MemberLeft`, which travels there and not here.
    if let Err(err) = node_client.sync_namespace(namespace.to_bytes()).await {
        warn!(
            ?err,
            ?namespace,
            "account-follow: failed to pull a namespace before unfollowing it"
        );
    }
    unfollow(node_client, namespace).await;
}

async fn unfollow(node_client: &NodeClient, namespace: ContextGroupId) {
    match node_client
        .unsubscribe_namespace(namespace.to_bytes())
        .await
    {
        Ok(()) => info!(
            ?namespace,
            "account-follow: unfollowed a namespace the account left"
        ),
        Err(err) => warn!(
            ?err,
            ?namespace,
            "account-follow: failed to unfollow a namespace the account left"
        ),
    }
}

async fn follow(context_client: &ContextClient, namespace: ContextGroupId) {
    match context_client
        .follow_namespace(FollowNamespaceRequest { namespace })
        .await
    {
        Ok(()) => info!(
            ?namespace,
            "account-follow: following a namespace the account gained"
        ),
        Err(err) => warn!(
            ?err,
            ?namespace,
            "account-follow: failed to follow a namespace the account gained"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use calimero_account::{
        AccountGenesis, AccountId, AccountProof, DeviceCert, DeviceId, DeviceRevocation,
        KemPublicKey, SignedDeviceRevocation,
    };
    use calimero_context_client::local_governance::GroupOp;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::op_events::OpEvent;
    use calimero_governance_store::{
        op_events, AccountBindingRepository, AccountDeviceRegistry, AccountNamespaceSet,
        GroupKeyring, MembershipRepository, MetaRepository, NamespaceRepository,
        NodeDeviceRepository,
    };
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupTarget};
    use calimero_store::Store;
    use tokio::time::sleep;

    use super::{
        carry_into, follows_on_gain, handle_revocation_carry, namespaces_now_covered,
        namespaces_to_bind_into, namespaces_to_revoke_in, publish_sibling_link, run,
        signing_identity, unfollows_on_left,
    };
    use crate::test_support::actor;

    const ACCOUNT_NAMESPACE: [u8; 32] = [0xC1; 32];
    const OTHER_GROUP: [u8; 32] = [0xC9; 32];

    fn ns(seed: u8) -> ContextGroupId {
        ContextGroupId::from([seed; 32])
    }

    fn app(seed: u8) -> ApplicationId {
        ApplicationId::from([seed; 32])
    }

    /// This node as a DEVICE of an account whose root lives elsewhere, scoped to
    /// `applications` (empty is every application): the account namespace
    /// recorded by pairing, this node's device minted under the account's
    /// genesis, and its own certified row folded - the state a device is in once
    /// pairing has settled.
    ///
    /// A device and not a holder on purpose. `NodeDeviceRepository::account_namespace`
    /// answers a holder from its root derivation and only falls through to the
    /// stored row otherwise, so a holder fixture would exercise the wrong read.
    /// The account root comes back beside them because it lives nowhere in this
    /// store, and a caller that has to sign for the account has no other source.
    fn a_device_scoped_to(
        store: &Store,
        applications: &[ApplicationId],
    ) -> (ContextGroupId, DeviceId, PrivateKey) {
        let root_sk = PrivateKey::from([0x70; 32]);
        let genesis = AccountGenesis::new(root_sk.public_key());
        let account = genesis.account_id();
        let account_namespace = ContextGroupId::from(ACCOUNT_NAMESPACE);

        let devices = NodeDeviceRepository::new(store);
        devices
            .store_account_namespace(&account_namespace)
            .expect("record what the pairing named");
        let held = devices
            .ensure_enrolled_into(&[account_namespace], genesis)
            .expect("mint this node's device");

        let proof = AccountProof {
            genesis,
            chain: vec![],
            statement: DeviceCert::sign(
                &root_sk,
                account,
                held.device(),
                &PrivateKey::from([0x71; 32]).public_key(),
                &KemPublicKey::from([0x72; 32]),
                0,
                0,
            )
            .expect("the account root signs this device's certificate"),
        };
        let _recorded = AccountDeviceRegistry::new(store, account_namespace)
            .record(&proof, applications, 0)
            .expect("record this device in its account's registry");
        (account_namespace, held.device(), root_sk)
    }

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// A namespace this node takes part in, targeting `application`, as a
    /// founder leaves it: a meta row and a participation row.
    fn a_namespace_targeting(store: &Store, namespace: ContextGroupId, application: ApplicationId) {
        MetaRepository::new(store)
            .save(
                &namespace,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: application,
                        ..GroupTarget::default()
                    },
                    created_at: 1_700_000_000,
                    admin_identity: AccountId::from([0x01; 32]),
                    owner_identity: AccountId::from([0x01; 32]),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save the namespace metadata");
        let _identity = NamespaceRepository::new(store)
            .participate_in(&namespace)
            .expect("take part in the namespace");
    }

    /// A sibling of this account in the registry, scoped to `applications`.
    ///
    /// Takes the root key rather than reading one: this node is a device, and a
    /// device holds no account root to sign a sibling's certificate with.
    fn a_sibling_scoped_to(
        store: &Store,
        account_namespace: ContextGroupId,
        root_sk: &PrivateKey,
        seed: u8,
        applications: &[ApplicationId],
    ) -> DeviceId {
        let genesis = AccountGenesis::new(root_sk.public_key());
        let device = DeviceId::from([seed; 32]);
        let proof = AccountProof {
            genesis,
            chain: vec![],
            statement: DeviceCert::sign(
                root_sk,
                genesis.account_id(),
                device,
                &PrivateKey::from([seed; 32]).public_key(),
                &KemPublicKey::from([seed ^ 0xFF; 32]),
                0,
                0,
            )
            .expect("the account root signs a sibling's cert"),
        };
        let _recorded = AccountDeviceRegistry::new(store, account_namespace)
            .record(&proof, applications, 0)
            .expect("record the sibling");
        device
    }

    /// The scope decides. A namespace targeting an application this device may
    /// not speak for is one it must not start replicating.
    #[test]
    fn a_covered_gain_is_followed_and_an_uncovered_one_is_not() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[app(0x11)]);

        assert!(follows_on_gain(
            &store,
            account_namespace.to_bytes(),
            Some(app(0x11))
        ));
        assert!(!follows_on_gain(
            &store,
            account_namespace.to_bytes(),
            Some(app(0x22))
        ));
        assert!(
            !follows_on_gain(&store, account_namespace.to_bytes(), None),
            "a namespace whose target has not synced is reachable only by an unscoped device"
        );
    }

    /// A device with no registry row yet does nothing, and catches up when its
    /// own scope arrives.
    #[test]
    fn a_device_that_has_not_folded_its_own_scope_follows_nothing() {
        let store = store();
        let account_namespace = ContextGroupId::from(ACCOUNT_NAMESPACE);
        let devices = NodeDeviceRepository::new(&store);
        devices
            .store_account_namespace(&account_namespace)
            .expect("record what the pairing named");
        let _held = devices
            .ensure_enrolled_into(
                &[account_namespace],
                AccountGenesis::new(PrivateKey::from([0x70; 32]).public_key()),
            )
            .expect("mint this node's device");

        assert!(!follows_on_gain(
            &store,
            account_namespace.to_bytes(),
            Some(app(0x11))
        ));
    }

    /// Somebody else's account namespace is somebody else's business.
    #[test]
    fn an_event_from_another_group_is_ignored() {
        let store = store();
        let (_account_namespace, device, _root_sk) = a_device_scoped_to(&store, &[]);

        assert!(!follows_on_gain(&store, OTHER_GROUP, Some(app(0x11))));
        assert!(!unfollows_on_left(&store, OTHER_GROUP, ns(0x91)));
        assert_eq!(namespaces_now_covered(&store, OTHER_GROUP, device), vec![]);
    }

    /// The catch-up path: a device paired after the account gained its
    /// namespaces, or widened later, follows what it now may and nothing else.
    #[test]
    fn this_nodes_own_scope_arriving_follows_the_set_it_now_covers() {
        let store = store();
        let (account_namespace, device, _root_sk) = a_device_scoped_to(&store, &[app(0x11)]);
        let set = AccountNamespaceSet::new(&store, account_namespace);
        set.record(ContextGroupId::from([0x91; 32]), Some(app(0x11)))
            .expect("in scope");
        set.record(ContextGroupId::from([0x92; 32]), Some(app(0x22)))
            .expect("out of scope");
        set.record(ContextGroupId::from([0x93; 32]), None)
            .expect("target unknown");

        assert_eq!(
            namespaces_now_covered(&store, account_namespace.to_bytes(), device),
            vec![ContextGroupId::from([0x91; 32])]
        );
        assert_eq!(
            namespaces_now_covered(
                &store,
                account_namespace.to_bytes(),
                DeviceId::from([0xCA; 32])
            ),
            vec![],
            "another device's scope says nothing about what this one may follow"
        );
    }

    /// Nothing re-drives a gain that was applied while no listener was up: the
    /// certified event fires once, an applied op is never re-applied, and a
    /// namespace with no participation row is invisible to the startup sweep. So
    /// the listener sweeps the account's set itself, every time it starts.
    #[actix::test]
    async fn a_gain_applied_while_nothing_listened_is_followed_on_the_next_start() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[app(0x11)]);
        let missed = ContextGroupId::from([0x94; 32]);
        AccountNamespaceSet::new(&store, account_namespace)
            .record(missed, Some(app(0x11)))
            .expect("a gain that landed while this device had no listener");

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);

        let namespaces = NamespaceRepository::new(&store);
        for _ in 0..100 {
            if namespaces
                .participating_namespaces()
                .expect("read the participation rows")
                .contains(&missed)
            {
                listener.abort();
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
        panic!("the listener never followed a namespace its account had already gained");
    }

    /// A leave is unfollowed whether or not this device's own scope ever arrived.
    #[test]
    fn a_left_namespace_is_unfollowed_without_a_registry_row() {
        let store = store();
        let account_namespace = ContextGroupId::from(ACCOUNT_NAMESPACE);
        NodeDeviceRepository::new(&store)
            .store_account_namespace(&account_namespace)
            .expect("record what the pairing named");

        assert!(unfollows_on_left(
            &store,
            account_namespace.to_bytes(),
            ns(0x91)
        ));
    }

    /// A listener of this test's own, beside the one `Actor::started` spawned:
    /// that one is a process-global singleton another harness may have aborted.
    fn listen(store: &Store, harness: &actor::Harness) -> tokio::task::JoinHandle<()> {
        tokio::spawn(run(
            op_events::subscribe(),
            store.clone(),
            harness.node_client.clone(),
            Arc::clone(harness.context_client.ack_router()),
            harness.context_client.clone(),
        ))
    }

    fn topic(namespace: ContextGroupId) -> String {
        format!("ns/{}", hex::encode(namespace.to_bytes()))
    }

    /// This node has folded `namespace`'s governance, so its absent set row and
    /// absent member row are verdicts rather than the silence of an unsynced one.
    fn folded(store: &Store, namespace: ContextGroupId) {
        let mut handle = store.handle();
        handle
            .put(
                &calimero_store::key::NamespaceGovHead::new(namespace.to_bytes()),
                &calimero_store::key::NamespaceGovHeadValue {
                    sequence: 1,
                    dag_heads: vec![[0x01; 32]],
                },
            )
            .expect("record a governance head for it");
    }

    /// Poll until `done` holds over every unsubscribe seen so far, and hand back
    /// what was seen. The recorder drains on read, so the accumulation is here.
    async fn unsubscribes_until(
        harness: &mut actor::Harness,
        done: impl Fn(&[String]) -> bool,
    ) -> Vec<String> {
        let mut seen = Vec::new();
        for _ in 0..100 {
            seen.append(&mut harness.unsubscribed());
            if done(&seen) {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        seen
    }

    /// Unsubscribing IS unfollowing, so the arm is pinned on the topic the node
    /// client dropped rather than on the decision that reached it.
    #[actix::test]
    async fn a_leave_this_account_announced_drops_the_namespaces_topic() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let left = ContextGroupId::from([0x95; 32]);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        op_events::notify(OpEvent::AccountNamespaceLeft {
            group_id: account_namespace.to_bytes(),
            namespace: left,
        });

        let seen = unsubscribes_until(&mut harness, |seen| seen.contains(&topic(left))).await;
        listener.abort();
        assert!(
            seen.contains(&topic(left)),
            "a leave the account announced has to drop the namespace's topic"
        );
    }

    /// The other half of the start-up sweep. Nothing re-drives a leave applied
    /// while no listener was up, and the topic would stay subscribed forever.
    #[actix::test]
    async fn a_namespace_left_while_nothing_listened_is_unfollowed_on_the_next_start() {
        let store = store();
        let (_account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let left = ContextGroupId::from([0x96; 32]);
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&left)
            .expect("this node still takes part in it");
        folded(&store, left);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);

        let seen = unsubscribes_until(&mut harness, |seen| seen.contains(&topic(left))).await;
        listener.abort();
        assert!(
            seen.contains(&topic(left)),
            "the sweep never unfollowed a namespace its account had already left"
        );
    }

    /// A namespace this node has not synced answers "absent" to both reads: the
    /// set names nothing, and a missing member row reads as not a member. Only
    /// a folded namespace can say the account actually left.
    #[actix::test]
    async fn an_unsynced_namespace_is_kept_because_nothing_says_the_account_left() {
        let store = store();
        let (_account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);
        let namespaces = NamespaceRepository::new(&store);
        let unsynced = ContextGroupId::from([0x9A; 32]);
        let _unsynced = namespaces
            .participate_in(&unsynced)
            .expect("paired into it, and nothing about it has arrived yet");
        // A namespace that really was left, as the barrier: the sweep reaching
        // it is what says the sweep has run at all.
        let left = ContextGroupId::from([0x9B; 32]);
        let _left = namespaces.participate_in(&left).expect("takes part in it");
        folded(&store, left);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let seen = unsubscribes_until(&mut harness, |seen| seen.contains(&topic(left))).await;
        listener.abort();

        assert!(seen.contains(&topic(left)), "the sweep has to have run");
        assert!(
            !seen.contains(&topic(unsynced)),
            "a namespace this node has never folded is unsynced, not left"
        );
    }

    /// The member row is the last word. This namespace is folded here and the
    /// set omits it, so only "the account is still a member of it" keeps it.
    #[actix::test]
    async fn a_namespace_the_set_omits_is_kept_while_the_account_is_still_a_member() {
        let store = store();
        let (account_namespace, _device, root_sk) = a_device_scoped_to(&store, &[app(0x11)]);
        let account = AccountGenesis::new(root_sk.public_key()).account_id();
        let kept = ContextGroupId::from([0x97; 32]);
        let namespaces = NamespaceRepository::new(&store);
        let _identity = namespaces
            .participate_in(&kept)
            .expect("this node takes part in it");
        // Folded here, so the two conditions before the member row both pass and
        // the chain cannot short-circuit ahead of the one under test.
        folded(&store, kept);
        MembershipRepository::new(&store)
            .add_member(&kept, &account, GroupMemberRole::Member)
            .expect("and its account is still a member of it");

        // One barrier per half, so the assertion holds whichever runs first: the
        // follow half reaches `followed`, and the unfollow half drops `left`.
        let followed = ContextGroupId::from([0x98; 32]);
        AccountNamespaceSet::new(&store, account_namespace)
            .record(followed, Some(app(0x11)))
            .expect("in scope");
        let left = ContextGroupId::from([0x99; 32]);
        let _identity = namespaces
            .participate_in(&left)
            .expect("takes part in it, and the set names it nowhere");
        folded(&store, left);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        let mut seen = Vec::new();
        let mut swept = false;
        for _ in 0..100 {
            seen.append(&mut harness.unsubscribed());
            swept = seen.contains(&topic(left))
                && namespaces
                    .participating_namespaces()
                    .expect("read the participation rows")
                    .contains(&followed);
            if swept {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        listener.abort();

        assert!(swept, "both halves of the sweep have to have run");
        assert!(
            !seen.contains(&topic(kept)),
            "a namespace the account is still a member of must stay followed"
        );
    }

    /// A sibling's leave never cuts this device off its own account topic, which
    /// is where every other event it acts on arrives.
    #[test]
    fn the_account_namespace_itself_is_never_unfollowed() {
        let store = store();
        let (account_namespace, _device, _root_sk) = a_device_scoped_to(&store, &[]);

        assert!(!unfollows_on_left(
            &store,
            account_namespace.to_bytes(),
            account_namespace
        ));
    }

    /// A binding for `device` in `namespace`, as a link op leaves one.
    fn a_device_bound_in(
        store: &Store,
        namespace: ContextGroupId,
        root_sk: &PrivateKey,
        device: DeviceId,
    ) {
        let genesis = AccountGenesis::new(root_sk.public_key());
        let cert = DeviceCert::sign(
            root_sk,
            genesis.account_id(),
            device,
            &PrivateKey::from([0x75; 32]).public_key(),
            &KemPublicKey::from([0x76; 32]),
            0,
            0,
        )
        .expect("the account root signs the cert the binding records");
        let recorded = AccountBindingRepository::new(store)
            .apply_link(&namespace, &genesis, &[], &cert)
            .expect("write the binding");
        assert!(
            recorded.is_ok(),
            "the fixture's own link must be admissible"
        );
    }

    fn a_revocation_of(root_sk: &PrivateKey, device: DeviceId) -> SignedDeviceRevocation {
        let genesis = AccountGenesis::new(root_sk.public_key());
        SignedDeviceRevocation {
            genesis,
            chain: vec![],
            statement: DeviceRevocation::sign(root_sk, genesis.account_id(), device, 0)
                .expect("the account root signs the revocation"),
        }
    }

    /// The carry. Every namespace this node takes part in that still has the
    /// device live, and nothing else: not the account namespace where it already
    /// landed, not one that has already tombstoned it, not one this node is a
    /// stranger to, and nothing at all without a proof.
    #[test]
    fn a_proof_bearing_revocation_is_carried_only_where_the_device_is_still_live() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let still_live = ContextGroupId::from([0x84; 32]);
        let already_tombstoned = ContextGroupId::from([0x85; 32]);
        for namespace in [account_namespace, still_live, already_tombstoned] {
            let _identity = NamespaceRepository::new(&store)
                .participate_in(&namespace)
                .expect("take part");
        }
        let lost = DeviceId::from([0x66; 32]);
        for namespace in [account_namespace, still_live, already_tombstoned] {
            a_device_bound_in(&store, namespace, &root_sk, lost);
        }
        AccountBindingRepository::new(&store)
            .apply_revocation(&already_tombstoned, lost)
            .expect("tombstone one of them ahead of the carry");
        let proof = a_revocation_of(&root_sk, lost);

        assert_eq!(
            namespaces_to_revoke_in(&store, account_namespace.to_bytes(), lost, Some(&proof)),
            vec![still_live]
        );
        assert_eq!(
            namespaces_to_revoke_in(&store, account_namespace.to_bytes(), lost, None),
            Vec::<ContextGroupId>::new(),
            "an admin revocation carries no proof, and admin authority stops at its group"
        );
        assert_eq!(
            namespaces_to_revoke_in(&store, OTHER_GROUP, lost, Some(&proof)),
            Vec::<ContextGroupId>::new(),
            "a revocation folded on a project's DAG is this node's own publish coming back"
        );
    }

    /// The projection rule. A sibling reaches the namespaces this node takes
    /// part in that its scope covers, and nothing else - not one out of scope,
    /// not the account namespace, whose certified op is already the record
    /// there and whose unset target no scope could name.
    #[test]
    fn a_certified_sibling_is_bound_where_its_scope_reaches_and_nowhere_else() {
        let store = store();
        let (account_namespace, own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let covered = ContextGroupId::from([0x81; 32]);
        let elsewhere = ContextGroupId::from([0x82; 32]);
        a_namespace_targeting(&store, covered, app(0x11));
        a_namespace_targeting(&store, elsewhere, app(0x22));
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&account_namespace)
            .expect("this node takes part in its own account namespace");
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x63, &[app(0x11)]);

        let (cert, targets) =
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), sibling)
                .expect("the sibling has somewhere to go");

        assert_eq!(cert.device(), sibling);
        assert_eq!(targets, vec![covered]);
        assert!(
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), own_device).is_none(),
            "this node never publishes its own link; the holder that signed it did"
        );
        assert!(
            namespaces_to_bind_into(&store, OTHER_GROUP, sibling).is_none(),
            "the same op folded in a project namespace is this node's own publish coming back"
        );
    }

    /// An unscoped sibling reaches everything this node takes part in - except
    /// the account namespace, which an empty scope would otherwise sweep up.
    #[test]
    fn an_unscoped_sibling_still_skips_the_account_namespace() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let project = ContextGroupId::from([0x83; 32]);
        a_namespace_targeting(&store, project, app(0x11));
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&account_namespace)
            .expect("this node takes part in its own account namespace");
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x64, &[]);

        let (_cert, targets) =
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), sibling)
                .expect("an unscoped sibling belongs everywhere this node is");

        assert_eq!(targets, vec![project]);
    }

    /// A revoked id is spent everywhere, so there is nowhere left to carry it.
    /// The target's own tombstone is not the gate: a revoker publishes one only
    /// where it takes part, so a namespace it is a stranger to never hears.
    #[test]
    fn a_sibling_revoked_anywhere_is_projected_nowhere() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let project = ContextGroupId::from([0x85; 32]);
        a_namespace_targeting(&store, project, app(0x11));
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&account_namespace)
            .expect("this node takes part in its own account namespace");
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x66, &[]);
        AccountBindingRepository::new(&store)
            .apply_revocation(&account_namespace, sibling)
            .expect("tombstone the sibling where this node did hear of it");

        assert!(
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), sibling).is_none(),
            "a device revoked in one namespace this node takes part in must reach no other"
        );
    }

    /// The arm itself, driven the way the sweep tests drive it. Pinned on the
    /// topic the publish reached rather than on the decision behind it.
    #[actix::test]
    async fn a_sibling_certified_on_the_account_topic_is_published_into_a_namespace_here() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let project = ContextGroupId::from([0x86; 32]);
        a_namespace_targeting(&store, project, app(0x11));
        let _key_id = GroupKeyring::new(&store, project)
            .store_key(&[0x42; 32])
            .expect("hold this namespace's scope key, without which nothing is published");
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x67, &[app(0x11)]);

        let mut harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        op_events::notify(OpEvent::AccountDeviceCertified {
            group_id: account_namespace.to_bytes(),
            device: sibling,
        });

        let mut seen = Vec::new();
        for _ in 0..100 {
            seen.append(&mut harness.broadcast_topics());
            if seen.contains(&topic(project)) {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        listener.abort();

        assert!(
            seen.contains(&topic(project)),
            "the listener never published a certified sibling into the namespace this \
             node takes part in that its scope covers"
        );
    }

    /// The revoked-anywhere rule, re-read after the wait. A revocation that folds
    /// while the projection sleeps has to stop it publishing anything.
    #[actix::test]
    async fn a_sibling_revoked_during_the_wait_is_published_nowhere() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let project = ContextGroupId::from([0x8B; 32]);
        a_namespace_targeting(&store, project, app(0x11));
        let _key_id = GroupKeyring::new(&store, project)
            .store_key(&[0x42; 32])
            .expect("hold this namespace's scope key, without which nothing is published");
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&account_namespace)
            .expect("this node takes part in its own account namespace");
        let sibling = a_sibling_scoped_to(&store, account_namespace, &root_sk, 0x6B, &[app(0x11)]);
        let (cert, targets) =
            namespaces_to_bind_into(&store, account_namespace.to_bytes(), sibling)
                .expect("the decision is taken before the wait");
        let signer_sk =
            PrivateKey::from(signing_identity(&store, &targets).expect("an identity to sign with"));

        let mut harness = actor::over(store.clone()).await;
        let _started = harness.broadcast_topics();
        // Tombstoned where no target would see it, so only the re-read can refuse.
        AccountBindingRepository::new(&store)
            .apply_revocation(&account_namespace, sibling)
            .expect("the revocation folds while the projection waits");
        publish_sibling_link(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            &targets,
            &signer_sk,
            &cert,
        )
        .await;

        assert!(
            !harness.broadcast_topics().contains(&topic(project)),
            "a sibling revoked while the projection waited must reach no namespace"
        );
    }

    /// The re-read before each publish. A sibling that got there first between
    /// the decision and this publish leaves nothing here to withdraw.
    #[actix::test]
    async fn a_namespace_a_sibling_already_withdrew_from_is_not_published_into() {
        let store = store();
        let (_account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let still_live = ContextGroupId::from([0x88; 32]);
        let withdrawn = ContextGroupId::from([0x89; 32]);
        a_namespace_targeting(&store, still_live, app(0x11));
        a_namespace_targeting(&store, withdrawn, app(0x11));
        let lost = DeviceId::from([0x69; 32]);
        for namespace in [still_live, withdrawn] {
            a_device_bound_in(&store, namespace, &root_sk, lost);
        }
        // What a faster sibling's carry left behind after this node decided.
        AccountBindingRepository::new(&store)
            .apply_revocation(&withdrawn, lost)
            .expect("a faster sibling withdrew it here");
        let account = AccountGenesis::new(root_sk.public_key()).account_id();
        let op = GroupOp::AccountDeviceUnlinked {
            account,
            device: lost,
            proof: Some(a_revocation_of(&root_sk, lost)),
        };
        let signer_sk = PrivateKey::from(
            signing_identity(&store, &[still_live, withdrawn]).expect("an identity to sign with"),
        );

        let mut harness = actor::over(store.clone()).await;
        let _started = harness.broadcast_topics();
        carry_into(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            &[still_live, withdrawn],
            &signer_sk,
            lost,
            &op,
        )
        .await;

        let seen = harness.broadcast_topics();
        assert!(
            seen.contains(&topic(still_live)),
            "the namespace that still has the device linked must get the withdrawal"
        );
        assert!(
            !seen.contains(&topic(withdrawn)),
            "a namespace a sibling already withdrew from must get nothing at all"
        );
    }

    /// The arm itself, driven the way the sweep tests drive it: the listener
    /// folds a proof-bearing revocation and the tombstone lands where it must.
    #[actix::test]
    async fn a_revocation_on_the_account_topic_is_carried_into_a_namespace_here() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let elsewhere = ContextGroupId::from([0x8A; 32]);
        a_namespace_targeting(&store, elsewhere, app(0x11));
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&account_namespace)
            .expect("this node takes part in its own account namespace");
        let lost = DeviceId::from([0x6A; 32]);
        a_device_bound_in(&store, account_namespace, &root_sk, lost);
        a_device_bound_in(&store, elsewhere, &root_sk, lost);
        let account = AccountGenesis::new(root_sk.public_key()).account_id();

        let harness = actor::over(store.clone()).await;
        let listener = listen(&store, &harness);
        op_events::notify(OpEvent::DeviceRevoked {
            group_id: account_namespace.to_bytes(),
            account,
            device: lost,
            proof: Some(Box::new(a_revocation_of(&root_sk, lost))),
        });

        let bindings = AccountBindingRepository::new(&store);
        let mut carried = false;
        for _ in 0..100 {
            carried = bindings.is_revoked(&elsewhere, lost).expect("read");
            if carried {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        listener.abort();
        assert!(
            carried,
            "the listener never carried a proof-bearing revocation off the account topic"
        );
    }

    /// The carry through the real publish path and the real store: the tombstone
    /// lands in the namespace the revoker never reached, and not in the account
    /// namespace, where the revocation it folded already is.
    #[actix::test]
    async fn the_carry_tombstones_the_device_where_the_revoker_could_not_reach() {
        let store = store();
        let (account_namespace, _own_device, root_sk) = a_device_scoped_to(&store, &[]);
        let elsewhere = ContextGroupId::from([0x86; 32]);
        a_namespace_targeting(&store, elsewhere, app(0x11));
        let _identity = NamespaceRepository::new(&store)
            .participate_in(&account_namespace)
            .expect("this node takes part in its own account namespace");

        let lost = DeviceId::from([0x67; 32]);
        a_device_bound_in(&store, account_namespace, &root_sk, lost);
        a_device_bound_in(&store, elsewhere, &root_sk, lost);
        let proof = a_revocation_of(&root_sk, lost);
        let account = AccountGenesis::new(root_sk.public_key()).account_id();

        let harness = actor::over(store.clone()).await;
        handle_revocation_carry(
            &store,
            &harness.node_client,
            harness.context_client.ack_router(),
            account_namespace.to_bytes(),
            account,
            lost,
            proof,
        )
        .await;

        let bindings = AccountBindingRepository::new(&store);
        assert!(
            bindings.is_revoked(&elsewhere, lost).expect("read"),
            "the namespace the revoker never reached must hold the tombstone"
        );
        assert!(
            !bindings.is_revoked(&account_namespace, lost).expect("read"),
            "the account namespace is where the revocation came FROM; republishing there echoes"
        );
    }
}
