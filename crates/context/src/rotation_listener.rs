//! Drains the pending forward-secrecy rotations that self-leaves leave behind.
//!
//! # The hand-off
//!
//! A key rotation is minted by whoever PUBLISHES the op that triggers it. For an
//! admin-initiated `MemberRemoved` that works — the publisher stays in the group and
//! wraps the new key for everyone who remains. For a self-leave it cannot: the
//! publisher IS the leaver, and they would have to mint the very key they are being
//! cut off from (and would keep it). Peers reject a rotation from a non-admin anyway.
//!
//! So the two halves are split across nodes. `MemberLeft`'s apply records what is owed
//! (a replicated `GroupPendingKeyRotation` row, written on every node because the
//! apply is deterministic), and this listener — running on the REMAINING admins —
//! discharges it by publishing `GroupKeyRotated` with a fresh key wrapped for the
//! members who are still there.
//!
//! # No election, no quorum
//!
//! Every remaining admin reacts. If several rotate concurrently they mint DIFFERENT
//! keys, and that is fine: the keyring already converges on one — highest epoch, ties
//! broken by the larger key id, a total order over a hash and therefore the same
//! choice on every node. Safety survives the race because EVERY competing key excludes
//! the leaver: whichever wins, the leaver holds none of them. The only cost is
//! redundant envelopes on the wire, which the pending-row check below mostly avoids
//! anyway (the first rotation to apply clears the row, and the rest become no-ops).
//!
//! Electing a single rotator would add a coordination problem — and a liveness
//! dependency on whoever got elected — to solve something the crypto already solves.
//!
//! # Liveness
//!
//! Two triggers, because an event alone is not enough:
//!
//! - the live `MemberRemoved` op-event, for the common case; and
//! - a **startup sweep** of the persisted worklist, because an admin that was offline
//!   when the leave applied never saw the event. Without the sweep, a leave that
//!   happened while the only remaining admin was down would never be rotated at all.
//!
//! Both funnel into the same idempotent request, so they can overlap harmlessly.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use calimero_context_client::client::ContextClient;
use calimero_context_client::group::RotationDebt;
use calimero_governance_store::{
    op_events, op_events::OpEvent, PendingDeviceRotationRepository, PendingRotationRepository,
};
use calimero_store::Store;
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

use calimero_context_config::types::ContextGroupId;

struct HandleState {
    abort: AbortHandle,
}

static HANDLE: Mutex<Option<HandleState>> = Mutex::new(None);

/// Bumped on every successful `spawn`, so a caller can tell whether the
/// listener it started is still the one running.
///
/// The listener is a process-global singleton bound to one `Store`, and
/// `spawn` rebinds it. In production that is what you want — one node, one
/// store. Under a parallel test binary it means any test that boots a node
/// silently takes the listener away from whatever test was using it, and the
/// symptom lands far away: a pending rotation that nobody discharges, which
/// reads as a broken op-event → listener → rotate → publish chain.
///
/// Exposing the generation lets a test say which of the two it hit instead of
/// blaming the chain. See `#[serial(boot_test_node)]` on every test that boots
/// a node — that guard is what keeps this from happening, and this counter is
/// how a future gap in it announces itself.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The current listener generation. See [`GENERATION`].
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// Start the rotation listener. Returns immediately; it runs as a detached task.
///
/// Subscribes to `op_events` **synchronously, before** spawning, so no event fired
/// after this returns can be missed — the same race the TEE-admit listener closes.
///
/// Idempotent: a second call while one is running is a no-op.
pub fn spawn(store: Store, context_client: ContextClient) {
    let mut slot = HANDLE.lock().expect("rotation-listener HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.abort.is_finished()) {
        debug!("rotation listener already running; skipping re-spawn");
        return;
    }
    let rx = op_events::subscribe();
    let abort = tokio::spawn(async move {
        // Drain first. An admin that was offline when the leave applied has no event
        // coming, so the persisted worklist is the only thing that will ever tell it
        // there is a rotation owed.
        drain_backlog(&store, &context_client).await;
        drain_device_backlog(&store, &context_client).await;
        run(rx, store, context_client).await;
    })
    .abort_handle();
    *slot = Some(HandleState { abort });
    let _ = GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Abort the listener. For tests and graceful shutdown; safe if none is running.
pub fn shutdown() {
    if let Some(state) = HANDLE
        .lock()
        .expect("rotation-listener HANDLE poisoned")
        .take()
    {
        state.abort.abort();
    }
}

/// Discharge every rotation this node still owes, from the persisted worklist.
///
/// Best-effort: a group this node is not an admin of, or has no key for, is declined
/// inside the handler and left for a node that can do it.
async fn drain_backlog(store: &Store, context_client: &ContextClient) {
    let pending = match PendingRotationRepository::new(store).all_pending() {
        Ok(p) => p,
        Err(err) => {
            warn!(%err, "rotation listener: could not read the pending-rotation worklist");
            return;
        }
    };
    if pending.is_empty() {
        return;
    }
    info!(
        count = pending.len(),
        "rotation listener: draining pending key rotations left by earlier departures"
    );
    for (group_id, departed) in pending {
        rotate(
            context_client,
            group_id,
            RotationDebt::MemberDeparted(departed),
        )
        .await;
    }
}

/// The same drain for rotations owed by device revocations.
///
/// A separate worklist because it discharges differently — see
/// [`RotationDebt::DeviceRevoked`]. Draining it at startup is what stops the debt
/// being lost when every admin happened to be down as the revocation applied,
/// which is otherwise indistinguishable from having rotated.
async fn drain_device_backlog(store: &Store, context_client: &ContextClient) {
    let pending = match PendingDeviceRotationRepository::new(store).all_pending() {
        Ok(p) => p,
        Err(err) => {
            warn!(%err, "rotation listener: could not read the pending-device-rotation worklist");
            return;
        }
    };
    if pending.is_empty() {
        return;
    }
    info!(
        count = pending.len(),
        "rotation listener: draining pending key rotations left by earlier device revocations"
    );
    for (group_id, device) in pending {
        rotate(
            context_client,
            group_id,
            RotationDebt::DeviceRevoked(device),
        )
        .await;
    }
}

async fn run(
    mut rx: tokio::sync::broadcast::Receiver<OpEvent>,
    store: Store,
    context_client: ContextClient,
) {
    info!("rotation listener started");
    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                // A dropped event is not a dropped rotation: the worklist row is
                // durable, so the next restart's backlog drain still finds it.
                warn!(
                    skipped,
                    "rotation listener lagged; any missed rotation is still recorded in the \
                     persisted worklist and will be drained"
                );
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                info!("rotation listener: op-event channel closed; exiting");
                break;
            }
        };

        // `MemberRemoved` fires for BOTH an admin-initiated removal and a self-leave.
        // We only owe a rotation for the latter — and the pending row is exactly what
        // distinguishes them, because an admin's removal already carried its own
        // rotation and never wrote one. So the row check below is the whole filter.
        // Two shapes of debt, and they are NOT interchangeable: a departure excludes
        // the account by name, a revocation excludes nobody (the device is already
        // out of the recipient list, and its account still holds other devices).
        let (group_id, debt) = match event {
            OpEvent::MemberRemoved { group_id, member } => (
                ContextGroupId::from(group_id),
                RotationDebt::MemberDeparted(member),
            ),
            OpEvent::DeviceRevoked {
                group_id, device, ..
            } => (
                ContextGroupId::from(group_id),
                RotationDebt::DeviceRevoked(device),
            ),
            _ => continue,
        };

        // The row is the whole filter. `MemberRemoved` fires for an admin-initiated
        // removal too, and `DeviceRevoked` for an admin-published revocation — both
        // of which already carried their own rotation, whose apply cleared the row
        // before this ever runs.
        let pending = match debt {
            RotationDebt::MemberDeparted(member) => {
                PendingRotationRepository::new(&store).is_pending(&group_id, &member)
            }
            RotationDebt::DeviceRevoked(device) => {
                PendingDeviceRotationRepository::new(&store).is_pending(&group_id, &device)
            }
        };
        match pending {
            Ok(true) => {}
            Ok(false) => continue,
            Err(err) => {
                warn!(?group_id, %err, "rotation listener: pending-rotation lookup failed");
                continue;
            }
        }

        // Own task per event, so a slow publish never blocks the receive loop and lets
        // the bounded broadcast channel overflow. Rotation is idempotent, so a late or
        // duplicated task is harmless.
        let context_client = context_client.clone();
        tokio::spawn(async move {
            rotate(&context_client, group_id, debt).await;
        });
    }
}

/// Ask the actor to rotate. It re-checks eligibility (admin, not the leaver, still
/// owed) and declines quietly if this node is not the right one to act — so several
/// admins reacting to the same departure is expected, not a problem.
async fn rotate(context_client: &ContextClient, group_id: ContextGroupId, debt: RotationDebt) {
    let request = calimero_context_client::group::RotateGroupKeyRequest { group_id, debt };
    if let Err(err) = context_client.rotate_group_key(request).await {
        // Best-effort: the row survives, so a later event, another admin, or the next
        // startup drain retries. Losing a rotation is a forward-secrecy hole, so this
        // is warn-level and says so.
        warn!(
            ?group_id,
            ?debt,
            %err,
            "failed to rotate group key after a departure; the pending row remains and will \
             be retried (until it succeeds, the departed member can still read this group)"
        );
    }
}
