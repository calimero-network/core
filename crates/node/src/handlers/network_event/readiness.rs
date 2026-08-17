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
//!   `peer_summary`. A beacon that fails verification but carries a
//!   verifiable admission proof still triggers a governance pull (see
//!   [`beacon_admission_provable`]) without entering the cache.
//! - `handle_readiness_probe` forwards the probe to the manager which
//!   rate-limits the per-(peer, namespace) response at
//!   `BEACON_INTERVAL / 2` — see
//!   [`Handler<EmitOutOfCycleBeacon>`](crate::readiness::ReadinessManager).
use std::collections::HashMap;
use std::time::{Duration, Instant};

use actix::{AsyncContext, WrapFuture};
use calimero_context_client::local_governance::{ReadinessProbe, SignedReadinessBeacon};
use calimero_governance_store::governance_broadcast::{
    beacon_admission_provable, verify_readiness_beacon,
};
use calimero_governance_store::now_millis;
use libp2p::PeerId;
use tracing::{debug, info, warn};

use crate::readiness::{ApplyBeaconLocal, EmitOutOfCycleBeacon, MAX_BEACON_CLOCK_DRIFT_MS};
use crate::NodeManager;

/// Per-namespace debounce window for beacon-triggered governance syncs.
/// One beacon interval (~5s): a Ready peer beacons every ~5s, so without
/// this a behind-node would fire one sync per beacon per peer.
const NS_BEACON_SYNC_DEBOUNCE: Duration = Duration::from_secs(5);

/// True when a beacon shows the peer is ahead AND this node should act on
/// it — i.e. trigger a namespace governance sync.
///
/// - `local_has_state` — this node holds a non-empty local namespace
///   governance DAG head, i.e. it is an established member. Always eligible.
/// - `is_namespace_member` — this node holds a namespace identity but no
///   state yet. Also eligible, and that is a change: it used to sit the
///   round out on the grounds that "the join flow owns the initial
///   governance sync", and that firing here would pull ops before the key
///   arrived and leave undecryptable skeletons behind.
///
///   Both halves of that stopped being true. A join whose direct request
///   found no reachable peer RETURNS — recording membership and relying on
///   fallback, with no key and an empty head — so no join flow owns
///   anything afterwards, and the node is excluded from the one mechanism
///   that would have recovered it. Meanwhile `sync_namespace_from_peer`
///   now finishes with `recover_missing_group_keys` and, when it applied
///   anything, `drain_governance_pending_after_sync` (#2613): the key is
///   fetched on the same round as the ops, and anything buffered
///   undecryptable is drained once it lands. The skeleton hazard is closed
///   by the very path this guard was blocking.
///
///   Cost of being wrong is one debounced sync round (~5s) against a peer
///   that is genuinely ahead of us.
/// - `dag_head == [0u8; 32]` — the peer has applied nothing yet; never
///   sync towards an empty DAG.
/// - `head_op_present_locally` — the peer's advertised head op is already
///   in our DAG; we are caught up (or ahead).
fn beacon_indicates_divergence(
    local_has_state: bool,
    is_namespace_member: bool,
    dag_head: [u8; 32],
    head_op_present_locally: bool,
) -> bool {
    (local_has_state || is_namespace_member) && dag_head != [0u8; 32] && !head_op_present_locally
}

/// Which of the two beacon paths asked for a governance pull.
///
/// One value rather than an `Option<PeerId>` alongside the signer, because the
/// two would have to agree and nothing could enforce it — passing one peer as the
/// pull target and a different one as the beacon's signer would compile and then
/// pull from the wrong place. The signer is always known, so it is passed
/// unconditionally and this says only what to do with it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BeaconTrigger {
    /// The beacon did not verify, but its admission proof did. Only its signer
    /// holds the not-yet-gossiped join op, so the pull targets that signer and
    /// nobody else — and takes the provable path's own debounce slot.
    ProvableAdmission,
    /// The beacon verified. Any member that applied the op can serve it, so the
    /// pull prefers a subscriber and falls back to the signer.
    EstablishedMember,
}

/// Per-namespace debounce gate. Returns `true` (and records `now`) when
/// no beacon-triggered sync fired for `namespace_id` within
/// [`NS_BEACON_SYNC_DEBOUNCE`]; returns `false` otherwise.
///
/// Applied to whichever of the caller's two per-path maps is in play, so the
/// window is per (namespace, path) without ever being per peer.
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

/// Whether a beacon's self-reported wall-clock is close enough to ours for it
/// to be a live signal rather than a replayed one.
///
/// Reuses [`MAX_BEACON_CLOCK_DRIFT_MS`], symmetrically. `ReadinessCache::insert`
/// only needs the far-future half of that bound because its per-peer monotonic
/// filter already discards old beacons; a beacon that never reaches the cache
/// has no stored entry to compare against, so it needs both halves.
fn beacon_ts_within_drift(ts_millis: u64, now_ms: u64) -> bool {
    ts_millis.abs_diff(now_ms) <= MAX_BEACON_CLOCK_DRIFT_MS
}

pub(super) fn handle_readiness_beacon(
    manager: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    peer_id: PeerId,
    beacon: SignedReadinessBeacon,
) {
    if !verify_readiness_beacon(&manager.datastore, &beacon) {
        // Pull from a provable non-member instead of dropping it, targeting the
        // signer: it is the only peer that holds its own not-yet-gossiped join op.
        if beacon_ts_within_drift(beacon.ts_millis, now_millis())
            && beacon_admission_provable(&manager.datastore, &beacon)
        {
            spawn_beacon_divergence_sync(
                manager,
                ctx,
                beacon.namespace_id.to_bytes(),
                beacon.dag_head,
                BeaconTrigger::ProvableAdmission,
                peer_id,
            );
            return;
        }
        // Last resort before the drop: this node may be the one the beacon
        // could have rescued.
        //
        // A member that joined without ever reaching a mesh peer holds its
        // membership row but no key and no governance ops. It cannot verify ANY
        // beacon — `verify_readiness_beacon` requires the signer to be a known
        // member, and knowing members requires the very governance state it is
        // missing — so it drops every one and nothing it hears can move it
        // forward. Meanwhile the beacons themselves are proof of what it needs:
        // a namespace peer is up, reachable, and talking to us right now.
        //
        // Acting on that trusts nothing new. The beacon is still refused — no
        // membership row, no cache entry, none of its claims are believed. We
        // use only the fact of its arrival to pick a peer, then pull through
        // the ordinary authenticated path where every governance op must still
        // validate on apply. The alternative, relaxing the verification above,
        // would widen what a stranger can assert about membership; this does
        // not.
        //
        // Deliberately NOT routed through `spawn_beacon_divergence_sync`: its
        // `local_has_state` guard skips a namespace with no non-zero DAG head,
        // which is exactly this node, and that guard is load-bearing for more
        // than its comment describes — relaxing it regressed a batch of
        // app-migration scenarios. So this arm does its own narrow pull and
        // leaves the guard alone.
        let stranded_ns = beacon.namespace_id.to_bytes();
        let stranded = matches!(
            calimero_governance_store::namespace_groups_member_but_keyless(
                &manager.datastore,
                stranded_ns.into(),
            ),
            Ok(ref groups) if !groups.is_empty()
        );
        if stranded && beacon_ts_within_drift(beacon.ts_millis, now_millis()) {
            let allowed = {
                let mut guard = manager
                    .ns_stranded_beacon_sync_debounce
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                debounce_allows_sync(&mut guard, stranded_ns, Instant::now())
            };
            if allowed {
                debug!(
                    %peer_id,
                    namespace_id = %hex::encode(beacon.namespace_id.as_bytes()),
                    "stranded member: unverifiable beacon signals a reachable peer, pulling from it"
                );
                let sync_manager = manager.managers.sync.clone();
                let _ignored = ctx.spawn(
                    async move {
                        let ops = sync_manager
                            .sync_namespace_from_peer(stranded_ns, Some(peer_id), None)
                            .await;
                        debug!(
                            %peer_id,
                            namespace_id = %hex::encode(stranded_ns),
                            ops,
                            "stranded-member recovery: pulled governance from beacon signer"
                        );
                        sync_manager
                            .recover_missing_group_keys(stranded_ns, Some(peer_id))
                            .await;
                        debug!(
                            %peer_id,
                            namespace_id = %hex::encode(stranded_ns),
                            "stranded-member recovery: key pull complete"
                        );
                    }
                    .into_actor(manager),
                );
                return;
            }
        }
        debug!(
            namespace_id = %hex::encode(beacon.namespace_id.as_bytes()),
            "ReadinessBeacon failed verification; dropping"
        );
        return;
    }
    let namespace_id = beacon.namespace_id.to_bytes();
    let peer_pubkey = beacon.peer_pubkey;
    let applied_through = beacon.applied_through;
    let strong = beacon.strong;
    manager.readiness_cache.insert(&beacon);
    // Wake any `await_first_fresh_beacon` waiters for this namespace
    // (Phase 8.1). Must run AFTER `cache.insert` so a waiter that
    // re-checks the cache on wakeup sees the new entry.
    manager.readiness_notify.notify(namespace_id);
    debug!(
        namespace_id = %hex::encode(namespace_id),
        peer = %peer_pubkey,
        applied_through,
        strong,
        "readiness beacon received"
    );

    spawn_beacon_divergence_sync(
        manager,
        ctx,
        namespace_id,
        beacon.dag_head,
        BeaconTrigger::EstablishedMember,
        peer_id,
    );

    if let Some(addr) = &manager.readiness_addr {
        addr.do_send(ApplyBeaconLocal { namespace_id });
    }
}

/// Receiver-side anti-entropy. The beacon advertises the peer's namespace
/// governance DAG head; if that head names an op we have not applied, the peer
/// is ahead and we pull the namespace DAG from it via the real governance sync
/// protocol (ops applied in DAG order, side-effects run). A spurious sync is
/// only wasted work, never wrong state.
///
/// The debounce slot is stamped *inside* the spawned future, after the DAG read
/// confirms divergence - never at receive time. A beacon from an
/// already-caught-up peer must not burn the per-namespace budget and suppress a
/// genuinely-divergent beacon from another peer for the next
/// [`NS_BEACON_SYNC_DEBOUNCE`] window.
///
/// `source` targets the pull at the beacon's own publisher. It is `Some` only on
/// the provable non-member path, where the advertised head op exists nowhere
/// else yet: that peer's join op has reached no other member, so pulling from an
/// arbitrary mesh peer would query someone who cannot serve it while still
/// burning the debounce slot. `None` keeps the established-member path on the
/// mesh-peer pull, where any member that applied the op can serve it.
///
/// The two paths debounce independently - `ns_beacon_sync_debounce` against
/// `ns_provable_beacon_sync_debounce`, see those field docs - and the provable
/// slot is released when the targeted pull returns nothing: a peer that cannot
/// serve the head it advertised forfeits the window it took.
fn spawn_beacon_divergence_sync(
    manager: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    namespace_id: [u8; 32],
    dag_head: [u8; 32],
    trigger: BeaconTrigger,
    beacon_peer: PeerId,
) {
    let datastore = manager.datastore.clone();
    let sync_manager = manager.managers.sync.clone();
    let debounce = match trigger {
        BeaconTrigger::ProvableAdmission => manager.ns_provable_beacon_sync_debounce.clone(),
        BeaconTrigger::EstablishedMember => manager.ns_beacon_sync_debounce.clone(),
    };
    let _ignored = ctx.spawn(
        async move {
            let handle = datastore.handle();
            // Local namespace governance state — at least one non-zero
            // DAG head. An empty/absent head (or only the `[0u8; 32]`
            // sentinel) means we are still bootstrapping or mid-join —
            // skip (see `beacon_indicates_divergence`): the join flow owns
            // the initial sync, and firing here races the join handshake.
            let local_has_state =
                match handle.get(&calimero_store::key::NamespaceGovHead::new(namespace_id)) {
                    Ok(Some(head)) => head.dag_heads.iter().any(|h| *h != [0u8; 32]),
                    Ok(None) => false,
                    Err(err) => {
                        // A failed read is datastore-level and unexpected.
                        warn!(
                            ?err,
                            namespace_id = %hex::encode(namespace_id),
                            "beacon-divergence: namespace head read failed; skipping sync"
                        );
                        return;
                    }
                };
            // `dag_head` is the beacon peer's namespace governance DAG
            // head — an op `delta_id`, which is exactly the second
            // component of the `NamespaceGovOp` store key. A point `get`
            // therefore tests whether we have applied that op locally.
            let head_op_present = match handle.get(&calimero_store::key::NamespaceGovOp::new(
                namespace_id,
                dag_head,
            )) {
                Ok(present) => present.is_some(),
                Err(err) => {
                    // A failed read is datastore-level and unexpected — do
                    // NOT trigger a sync on unknown state. The next beacon
                    // (~5s) retries.
                    warn!(
                        ?err,
                        namespace_id = %hex::encode(namespace_id),
                        "beacon-divergence: local DAG read failed; skipping sync"
                    );
                    return;
                }
            };
            drop(handle);
            // A namespace identity is what makes this node a member of the
            // namespace at all — it is written by the join, before any key or
            // governance state exists. So it is exactly the signal that
            // separates "joined, but stranded with nothing" from "not our
            // namespace", which must still be ignored.
            let is_namespace_member =
                calimero_governance_store::NamespaceRepository::new(&datastore)
                    .identity_record(&calimero_context_config::types::ContextGroupId::from(
                        namespace_id,
                    ))
                    .map(|record| record.is_some())
                    .unwrap_or(false);
            if !beacon_indicates_divergence(
                local_has_state,
                is_namespace_member,
                dag_head,
                head_op_present,
            ) {
                return;
            }
            // Divergence confirmed. Claim the debounce slot atomically;
            // if another beacon already triggered a sync for this
            // namespace on this path within the window, skip. The guard
            // is dropped before the `.await` below, so the lock is never
            // held across an await point.
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
                ?trigger,
                %beacon_peer,
                "beacon advertises an unknown namespace DAG head; \
                 triggering governance sync"
            );
            match trigger {
                // Issues the same `NamespaceBackfillRequest` with empty
                // `delta_ids` ("give me everything"), but against the beacon's
                // signer directly rather than an arbitrary subscriber.
                BeaconTrigger::ProvableAdmission => {
                    let ops = sync_manager
                        .sync_namespace_from_peer(namespace_id, Some(beacon_peer), None)
                        .await;
                    if ops == 0 {
                        // The pull delivered nothing, so give the slot back
                        // rather than making the next genuine beacon wait it out.
                        let _ = debounce
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .remove(&namespace_id);
                        debug!(
                            namespace_id = %hex::encode(namespace_id),
                            %beacon_peer,
                            "provable-admission pull returned no ops; released the debounce slot"
                        );
                    }
                }
                // The established-member path: prefer a subscriber, because any
                // member that applied the op can serve it. But name the beacon's
                // own signer as the fallback, because discovery can come up empty
                // while that signer is demonstrably reachable — its beacon just
                // verified, which is proof of membership, of being caught up, and
                // of being able to reach us, all at once. Asking an empty
                // subscriber table and giving up is how a node stays stranded with
                // the answer talking to it every few seconds.
                //
                // Run inline rather than queueing through `NodeClient::sync_namespace`,
                // because the queue carries a namespace id and nothing else — there
                // is nowhere to put the fallback peer without widening that channel
                // and every constructor of it.
                //
                // The slot is deliberately NOT released on a zero-op pull, unlike
                // the arm above. That arm's pull is targeted, so an empty result is
                // a fact about the peer that beaconed; this one usually goes to
                // whichever subscriber discovery picked, so an empty result says
                // nothing about the beacon's signer and releasing on it would let
                // every beacon re-trigger a pull. The debounce window IS the retry
                // interval here: the next beacon after it re-evaluates from scratch.
                BeaconTrigger::EstablishedMember => {
                    let ops = sync_manager
                        .sync_namespace_from_peer(namespace_id, None, Some(beacon_peer))
                        .await;
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        %beacon_peer,
                        ops,
                        "beacon-triggered namespace governance pull finished"
                    );
                }
            }
        }
        .into_actor(manager),
    );
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
            namespace_id: probe.namespace_id.to_bytes(),
            requesting_peer: peer_id,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divergence_true_when_head_op_absent() {
        // Established member (has local state), peer's head op missing.
        assert!(beacon_indicates_divergence(true, false, [7u8; 32], false));
    }

    #[test]
    fn divergence_false_when_head_op_present() {
        assert!(!beacon_indicates_divergence(true, true, [7u8; 32], true));
    }

    #[test]
    fn divergence_false_for_zero_head() {
        // A peer that has applied nothing advertises a zero head; never
        // sync towards an empty DAG even though the op is "absent".
        assert!(!beacon_indicates_divergence(true, true, [0u8; 32], false));
    }

    #[test]
    fn divergence_false_for_a_stranger_to_this_namespace() {
        // No local state AND not a member: someone else's namespace. Ignore it.
        assert!(!beacon_indicates_divergence(false, false, [7u8; 32], false));
    }

    /// **A member stranded with no state still pulls.**
    ///
    /// This is the case that deadlocked `group-join-mesh-not-ready`. A join
    /// whose direct request found no reachable peer returns anyway — membership
    /// recorded, no key, empty DAG head — so nothing owns the initial
    /// governance sync afterwards. Excluding it here left the node knowing it
    /// was behind (`we_missing=1` on every 30s heartbeat) with no way to act on
    /// it, while the peer that had the op would not dial back because the node
    /// was not subscribed to a context topic it had no key to join.
    #[test]
    fn divergence_true_for_a_member_with_no_state_yet() {
        assert!(beacon_indicates_divergence(false, true, [7u8; 32], false));
    }

    #[test]
    fn divergence_false_for_a_stateless_member_when_the_peer_has_nothing() {
        // Still never sync towards an empty DAG, member or not.
        assert!(!beacon_indicates_divergence(false, true, [0u8; 32], false));
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
    fn beacon_ts_drift_window_is_symmetric() {
        let now = 1_700_000_000_000u64;
        assert!(beacon_ts_within_drift(now, now));
        assert!(beacon_ts_within_drift(now - MAX_BEACON_CLOCK_DRIFT_MS, now));
        assert!(beacon_ts_within_drift(now + MAX_BEACON_CLOCK_DRIFT_MS, now));
        // A replayed beacon from outside the window, and a far-future one.
        assert!(!beacon_ts_within_drift(
            now - MAX_BEACON_CLOCK_DRIFT_MS - 1,
            now
        ));
        assert!(!beacon_ts_within_drift(
            now + MAX_BEACON_CLOCK_DRIFT_MS + 1,
            now
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
