//! Receiver-side handlers for the namespace-topic readiness variants.
//!
//! Phase 7.3 of the three-phase governance contract (#2237).
//!
//! - `handle_readiness_beacon` verifies the signature + namespace
//!   membership via [`verify_readiness_beacon`], inserts the beacon
//!   into the shared [`ReadinessCache`] (the cache is internally
//!   synchronised, so we bypass the `ReadinessManager` mailbox here
//!   to avoid a per-beacon hop), then notifies the manager via
//!   [`ApplyBeaconLocal`] so the FSM can re-evaluate against the new
//!   `peer_summary`.
//! - `handle_readiness_probe` forwards the probe to the manager which
//!   rate-limits the per-(peer, namespace) response at
//!   `BEACON_INTERVAL / 2` — see
//!   [`Handler<EmitOutOfCycleBeacon>`](crate::readiness::ReadinessManager).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use actix::{AsyncContext, WrapFuture};
use calimero_context::governance_broadcast::verify_readiness_beacon;
use calimero_context_client::local_governance::{ReadinessProbe, SignedReadinessBeacon};
use libp2p::PeerId;
use tracing::{debug, info, warn};

use crate::readiness::{ApplyBeaconLocal, EmitOutOfCycleBeacon};
use crate::NodeManager;

/// Per-namespace debounce window for beacon-triggered governance syncs.
/// One beacon interval (~5s): a Ready peer beacons every ~5s, so without
/// this a behind-node would fire one sync per beacon per peer.
const NS_BEACON_SYNC_DEBOUNCE: Duration = Duration::from_secs(5);

/// True if the beacon's advertised DAG head names a namespace governance
/// op this node has not applied locally — i.e. the beaconing peer is
/// ahead and we should pull the namespace governance DAG from it.
///
/// A zero head (`[0u8; 32]`) means the peer has applied nothing yet;
/// never sync towards an empty DAG.
fn beacon_indicates_divergence(dag_head: [u8; 32], head_op_present_locally: bool) -> bool {
    dag_head != [0u8; 32] && !head_op_present_locally
}

/// Per-namespace debounce gate. Returns `true` (and records `now`) when
/// no beacon-triggered sync fired for `namespace_id` within
/// [`NS_BEACON_SYNC_DEBOUNCE`]; returns `false` otherwise.
fn debounce_allows_sync(
    debounce: &mut HashMap<[u8; 32], Instant>,
    namespace_id: [u8; 32],
    now: Instant,
) -> bool {
    match debounce.get(&namespace_id) {
        Some(last) if now.duration_since(*last) < NS_BEACON_SYNC_DEBOUNCE => false,
        _ => {
            let _ = debounce.insert(namespace_id, now);
            true
        }
    }
}

pub(super) fn handle_readiness_beacon(
    manager: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    _peer_id: PeerId,
    beacon: SignedReadinessBeacon,
) {
    if !verify_readiness_beacon(&manager.datastore, &beacon) {
        debug!(
            namespace_id = %hex::encode(beacon.namespace_id),
            "ReadinessBeacon failed verification; dropping"
        );
        return;
    }
    let namespace_id = beacon.namespace_id;
    let peer_pubkey = beacon.peer_pubkey;
    let applied_through = beacon.applied_through;
    let strong = beacon.strong;
    manager.readiness_cache.insert(&beacon);
    // Wake any `await_first_fresh_beacon` waiters for this namespace
    // (Phase 8.1). Must run AFTER `cache.insert` so a waiter that
    // re-checks the cache on wakeup sees the new entry.
    manager.readiness_notify.notify(namespace_id);
    info!(
        namespace_id = %hex::encode(namespace_id),
        peer = %peer_pubkey,
        applied_through,
        strong,
        "readiness beacon received"
    );

    // #2367 — receiver-side anti-entropy. The beacon advertises the
    // peer's namespace governance DAG head; if that head names an op we
    // have not applied, the peer is ahead and we pull the namespace DAG
    // from it via the real governance sync protocol (ops applied in DAG
    // order, side-effects run). A spurious sync is only wasted work,
    // never wrong state.
    //
    // The debounce slot is stamped *inside* the spawned future, after
    // the DAG read confirms divergence — never at receive time. A beacon
    // from an already-caught-up peer must not burn the per-namespace
    // budget and suppress a genuinely-divergent beacon from another peer
    // for the next `NS_BEACON_SYNC_DEBOUNCE` window.
    let dag_head = beacon.dag_head;
    let datastore = manager.datastore.clone();
    let node_client = manager.clients.node.clone();
    let debounce = manager.ns_beacon_sync_debounce.clone();
    let _ignored = ctx.spawn(
        async move {
            let head_op_present = {
                let handle = datastore.handle();
                let op_key = calimero_store::key::NamespaceGovOp::new(namespace_id, dag_head);
                match handle.get(&op_key) {
                    Ok(present) => present.is_some(),
                    Err(err) => {
                        // Unknown local state — do NOT trigger a sync
                        // on a failed read. The next beacon (~5s) retries.
                        debug!(
                            ?err,
                            namespace_id = %hex::encode(namespace_id),
                            "beacon-divergence: local DAG read failed; skipping sync"
                        );
                        return;
                    }
                }
            };
            if !beacon_indicates_divergence(dag_head, head_op_present) {
                return;
            }
            // Divergence confirmed. Claim the debounce slot atomically;
            // if another beacon already triggered a sync for this
            // namespace within the window, skip. The guard is dropped
            // before the `.await` below — the lock is never held across
            // an await point.
            {
                let mut guard = debounce
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !debounce_allows_sync(&mut guard, namespace_id, Instant::now()) {
                    return;
                }
            }
            info!(
                namespace_id = %hex::encode(namespace_id),
                dag_head = %hex::encode(dag_head),
                "beacon advertises an unknown namespace DAG head; \
                 triggering governance sync"
            );
            if let Err(err) = node_client.sync_namespace(namespace_id).await {
                warn!(
                    ?err,
                    namespace_id = %hex::encode(namespace_id),
                    "beacon-triggered namespace governance sync failed"
                );
            }
        }
        .into_actor(manager),
    );

    if let Some(addr) = &manager.readiness_addr {
        addr.do_send(ApplyBeaconLocal { namespace_id });
    }
}

pub(super) fn handle_readiness_probe(
    manager: &mut NodeManager,
    _ctx: &mut actix::Context<NodeManager>,
    peer_id: PeerId,
    probe: ReadinessProbe,
) {
    // Forward to the manager so it can rate-limit per-(peer, namespace).
    // No verification needed at this layer — `ReadinessProbe` is
    // unsigned (it carries a 16-byte nonce only), and the
    // `EmitOutOfCycleBeacon` handler is the choke point that prevents
    // probe-driven amplification regardless of probe content.
    if let Some(addr) = &manager.readiness_addr {
        addr.do_send(EmitOutOfCycleBeacon {
            namespace_id: probe.namespace_id,
            requesting_peer: peer_id,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divergence_true_when_head_op_absent() {
        assert!(beacon_indicates_divergence([7u8; 32], false));
    }

    #[test]
    fn divergence_false_when_head_op_present() {
        assert!(!beacon_indicates_divergence([7u8; 32], true));
    }

    #[test]
    fn divergence_false_for_zero_head() {
        // A peer that has applied nothing advertises a zero head; never
        // sync towards an empty DAG even though the op is "absent".
        assert!(!beacon_indicates_divergence([0u8; 32], false));
    }

    #[test]
    fn debounce_allows_first_then_blocks_within_window() {
        let mut d: HashMap<[u8; 32], Instant> = HashMap::new();
        let t0 = Instant::now();
        assert!(debounce_allows_sync(&mut d, [1u8; 32], t0));
        // Second beacon 1s later — inside the 5s window — is blocked.
        assert!(!debounce_allows_sync(
            &mut d,
            [1u8; 32],
            t0 + Duration::from_secs(1)
        ));
    }

    #[test]
    fn debounce_reallows_after_window() {
        let mut d: HashMap<[u8; 32], Instant> = HashMap::new();
        let t0 = Instant::now();
        assert!(debounce_allows_sync(&mut d, [1u8; 32], t0));
        assert!(debounce_allows_sync(
            &mut d,
            [1u8; 32],
            t0 + NS_BEACON_SYNC_DEBOUNCE + Duration::from_millis(1)
        ));
    }

    #[test]
    fn debounce_is_per_namespace() {
        let mut d: HashMap<[u8; 32], Instant> = HashMap::new();
        let t0 = Instant::now();
        assert!(debounce_allows_sync(&mut d, [1u8; 32], t0));
        // Different namespace — independent budget, still allowed.
        assert!(debounce_allows_sync(&mut d, [2u8; 32], t0));
    }
}
